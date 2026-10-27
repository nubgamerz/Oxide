//! Connecting Claude Code and Claude Desktop end to end, in a profile of its own (dev plan
//! 17.3, 23.3): never the user's files.
//!
//! `claude_link_check <daw.exe> <oxide-mcp.exe> <audio.wav>`
//!
//! In `out/check/claude-link`: Claude Code's folder (given to the real `claude` CLI as
//! `CLAUDE_CONFIG_DIR`, which keeps it off `~\.claude.json`), and Claude Desktop installed both
//! ways (the .exe installer's `%APPDATA%\Claude` and a Store package's `LocalCache`), each
//! config with servers and settings of "the user's". It configures both clients for one port
//! and token and checks the files (ours added, every other byte kept, the backup, the bridge
//! copied, Claude Code given the same bridge), sees the status turn to "another port" when the
//! port changes and reconfigures. Then, with the app closed, it starts the bridge exactly as
//! each client's config says to run it (Desktop's and Claude Code's) and hears it answer
//! initialize, list the registry's tools and answer a call with "isn't running" as a tool
//! error; starts the app on the new port and, on the same bridge processes, gets a call
//! through; closes the app and hears the calls say it isn't running again. It removes both,
//! sees an HTTP entry added by the real CLI (as before 23.3) read as such and Reconfigure move
//! it to the bridge, and finally checks that the user's own Claude configs were not touched.
//! Without `claude` on the PATH, Claude Code's half is skipped.
//!
//! Prints `RESULT: PASS …` / `FAIL: …` lines; exits 1 on a failure.

use mcp_link::bridge::{Catalog, NOT_RUNNING, RUNNING_NOTE};
use mcp_link::clients::{self, Act, Client, Locations, Status, Wanted};
use mcp_link::json_edit;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const FIRST_PORT: u16 = 7894;
const SECOND_PORT: u16 = 7895;

const CLASSIC: &str = "{\n  \"globalShortcut\": \"Ctrl+Space\",\n  \"mcpServers\": {\n    \"filesystem\": {\n      \"command\": \"npx\",\n      \"args\": [\n        \"-y\",\n        \"@modelcontextprotocol/server-filesystem\",\n        \"C:\\\\Users\\\\me\\\\Music\"\n      ]\n    }\n  },\n  \"isUsingBuiltInNodeForMcp\": true\n}\n";

const STORE: &str = "{\r\n    \"preferences\": {\"menuBarEnabled\": false, \"quickEntryShortcut\": \"off\"},\r\n    \"mcpServers\": {\r\n        \"blender\": {\"command\": \"uvx\", \"args\": [\"blender-mcp\"]},\r\n        \"zz-last\": {\"command\": \"x.exe\", \"env\": {\"A\": \"1\"}}\r\n    }\r\n}";

const CODE: &str = "{\n  \"numStartups\": 7,\n  \"mcpServers\": {\n    \"rea\": {\n      \"type\": \"stdio\",\n      \"command\": \"rea.exe\",\n      \"args\": []\n    }\n  },\n  \"projects\": {}\n}\n";

struct Check {
    failed: bool,
}

impl Check {
    fn pass(&self, what: &str) {
        println!("RESULT: PASS {what}");
    }
    fn fail(&mut self, what: &str) {
        println!("FAIL: {what}");
        self.failed = true;
    }
    fn expect(&mut self, ok: bool, what: &str) -> bool {
        if ok {
            self.pass(what);
        } else {
            self.fail(what);
        }
        ok
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// The file without our entry, which must be the seed byte for byte.
fn without_ours(path: &Path) -> String {
    let text = read(path);
    json_edit::remove(&text, "mcpServers", clients::NAME)
        .ok()
        .flatten()
        .unwrap_or(text)
}

/// The user's own Claude configs, as they are now: Claude Code's user-scope servers (the file
/// itself changes whenever a Claude session runs) and each Desktop config's bytes.
fn users_own() -> Vec<(String, String)> {
    let mut seen = Vec::new();
    let Some(user) = std::env::current_exe()
        .ok()
        .and_then(|e| Locations::from_env(e.parent()?))
    else {
        return seen;
    };
    let code = user.code_config();
    let servers = json_edit::parse(&read(&code))
        .ok()
        .and_then(|v| v.get("mcpServers").cloned())
        .unwrap_or(Value::Null);
    seen.push((code.display().to_string(), servers.to_string()));
    for path in user.desktop_configs() {
        seen.push((path.display().to_string(), read(&path)));
    }
    seen
}

/// A bridge started as a client's config says to run it.
struct Bridge {
    /// Which config it came from, for the lines printed.
    name: String,
    child: Child,
    lines: Receiver<String>,
    /// Messages that answered nothing (notifications).
    notes: Vec<Value>,
}

impl Bridge {
    /// Start the `oxide` entry's program in `config` (Desktop's and Claude Code's alike).
    fn start(name: &str, config: &Path) -> Result<Self, String> {
        let value = json_edit::parse(&read(config))?;
        let entry = &value["mcpServers"][clients::NAME];
        let command = entry["command"].as_str().ok_or("no command")?;
        let args: Vec<&str> = entry["args"]
            .as_array()
            .ok_or("no args")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("{command}: {e}"))?;
        // Read on a thread, so reading can time out.
        let (send, lines) = std::sync::mpsc::channel();
        let out = child.stdout.take().expect("piped stdout");
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            name: name.to_string(),
            child,
            lines,
            notes: Vec::new(),
        })
    }

    fn send(&mut self, message: Value) {
        if let Some(stdin) = self.child.stdin.as_mut() {
            let _ = writeln!(stdin, "{message}");
            let _ = stdin.flush();
        }
    }

    /// The message answering `id` (others, such as notifications, kept in `notes`).
    fn answer(&mut self, id: i64, limit: Duration) -> Option<Value> {
        let end = Instant::now() + limit;
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            let line = self.lines.recv_timeout(left).ok()?;
            let message: Value = serde_json::from_str(&line).ok()?;
            if message.get("id").and_then(Value::as_i64) == Some(id) {
                return Some(message);
            }
            self.notes.push(message);
        }
        None
    }

    /// `tools/call get_project`: the text answered and whether it is a tool error (`None`
    /// for no answer or a protocol error).
    fn get_project(&mut self, id: i64) -> Option<(String, bool)> {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "get_project", "arguments": {}}}));
        let reply = self.answer(id, Duration::from_secs(60))?;
        let result = reply.get("result")?;
        Some((
            result["content"][0]["text"].as_str()?.to_string(),
            result["isError"] == true,
        ))
    }

    /// Close its stdin and wait for it to end.
    fn finish(mut self) -> bool {
        drop(self.child.stdin.take());
        let end = Instant::now() + Duration::from_secs(15);
        while Instant::now() < end {
            if let Ok(Some(status)) = self.child.try_wait() {
                return status.success();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        false
    }
}

fn initialize(id: i64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "claude_link_check", "version": "1"}}})
}

fn port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    )
    .is_ok()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [app, bridge, audio] = &args[..] else {
        eprintln!("usage: claude_link_check <daw.exe> <oxide-mcp.exe> <audio.wav>");
        std::process::exit(2);
    };
    let mut check = Check { failed: false };
    let before = users_own();

    let root = std::path::absolute("out/check/claude-link").expect("a working directory");
    let _ = std::fs::remove_dir_all(&root);
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default();
    let cli = clients::find_cli(std::env::var_os("PATH"), &home);
    let locations = Locations::profile(&root, cli.clone(), std::path::absolute(bridge).unwrap());
    match &cli {
        Some(cli) => println!(
            "claude: {} (CLAUDE_CONFIG_DIR={})",
            cli.display(),
            locations.code_config().parent().unwrap().display()
        ),
        None => println!("claude isn't on the PATH: Claude Code's half is skipped"),
    }

    // The profile: Claude Code with a server of its own, Desktop installed both ways.
    std::fs::create_dir_all(&locations.home).unwrap();
    let code_config = locations.code_config();
    std::fs::create_dir_all(code_config.parent().unwrap()).unwrap();
    std::fs::write(&code_config, CODE).unwrap();
    std::fs::create_dir_all(locations.local.join("AnthropicClaude")).unwrap();
    let classic = locations.roaming.join(r"Claude\claude_desktop_config.json");
    let store = locations
        .local
        .join(r"Packages\Claude_check1234\LocalCache\Roaming\Claude\claude_desktop_config.json");
    for (path, seed) in [(&classic, CLASSIC), (&store, STORE)] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, seed).unwrap();
    }
    check.expect(
        locations.desktop_configs() == vec![classic.clone(), store.clone()],
        "both of Desktop's configs found (installer and Store)",
    );

    let token = "5ca1ab1e".repeat(8);
    let first = Wanted {
        port: FIRST_PORT,
        token: Some(token.clone()),
    };
    let second = Wanted {
        port: SECOND_PORT,
        token: Some(token.clone()),
    };
    let code = cli.is_some();

    let d = clients::detect(&locations, &first);
    check.expect(
        d.desktop == Status::NotConfigured,
        &format!("Desktop starts not configured ({})", d.desktop.text()),
    );
    if code {
        check.expect(
            d.code == Status::NotConfigured,
            &format!("Claude Code starts not configured ({})", d.code.text()),
        );
    }

    // Configure.
    match clients::apply(&locations, Client::Desktop, Act::Configure, &first) {
        Ok(said) => check.expect(
            said.ends_with("Restart Claude Desktop to use it"),
            &format!("Desktop configured: {said}"),
        ),
        Err(e) => check.expect(false, &format!("Desktop configured: {e}")),
    };
    for (path, seed) in [(&classic, CLASSIC), (&store, STORE)] {
        let name = path.display();
        check.expect(
            without_ours(path) == seed,
            &format!("{name}: every other byte kept"),
        );
        let backup = path.with_file_name("claude_desktop_config.json.oxide-backup");
        check.expect(
            read(&backup) == seed,
            &format!("{name}: backup written first"),
        );
        let value = json_edit::parse(&read(path)).unwrap_or_default();
        check.expect(
            value["mcpServers"][clients::NAME] == clients::desktop_server(&locations, &first),
            &format!("{name}: runs the bridge with --port {FIRST_PORT} --token"),
        );
    }
    check.expect(
        std::fs::read(&locations.bridge).ok() == std::fs::read(bridge).ok(),
        &format!("the bridge copied to {}", locations.bridge.display()),
    );
    if code {
        match clients::apply(&locations, Client::Code, Act::Configure, &first) {
            Ok(said) => check.pass(&format!("Claude Code configured: {said}")),
            Err(e) => check.fail(&format!("Claude Code configured: {e}")),
        }
        let value = json_edit::parse(&read(&code_config)).unwrap_or_default();
        let entry = &value["mcpServers"][clients::NAME];
        let args = json!(["--port", FIRST_PORT.to_string(), "--token", token]);
        check.expect(
            entry["command"] == locations.bridge.to_string_lossy().as_ref()
                && entry["args"] == args
                && entry.get("url").is_none(),
            &format!(
                "Claude Code's user config runs the bridge {} with --port {FIRST_PORT} --token \
                 (stdio, no URL)",
                locations.bridge.display()
            ),
        );
        check.expect(
            value["mcpServers"]["rea"]["command"] == "rea.exe" && value["numStartups"] == 7,
            "Claude Code's other server and settings kept",
        );
    }
    let d = clients::detect(&locations, &first);
    check.expect(
        d.desktop == Status::Configured { port: FIRST_PORT },
        &format!("Desktop: {}", d.desktop.text()),
    );
    if code {
        check.expect(
            d.code == Status::Configured { port: FIRST_PORT },
            &format!("Claude Code: {}", d.code.text()),
        );
        check.expect(
            clients::apply(&locations, Client::Code, Act::Configure, &first).is_err(),
            "Configure refused for Claude Code once configured",
        );
    }
    check.expect(
        clients::apply(&locations, Client::Desktop, Act::Configure, &first).as_deref()
            == Ok("Claude Desktop: nothing to change"),
        "Configure changes nothing in Desktop once configured",
    );

    // The port changes: "another port" until Reconfigure.
    let d = clients::detect(&locations, &second);
    check.expect(
        d.desktop
            == Status::OtherPort {
                port: Some(FIRST_PORT),
            },
        &format!("after the port change, Desktop: {}", d.desktop.text()),
    );
    if code {
        check.expect(
            d.code
                == Status::OtherPort {
                    port: Some(FIRST_PORT),
                },
            &format!("after the port change, Claude Code: {}", d.code.text()),
        );
        if let Err(e) = clients::apply(&locations, Client::Code, Act::Reconfigure, &second) {
            check.fail(&format!("Claude Code reconfigured: {e}"));
        }
    }
    if let Err(e) = clients::apply(&locations, Client::Desktop, Act::Reconfigure, &second) {
        check.fail(&format!("Desktop reconfigured: {e}"));
    }
    let d = clients::detect(&locations, &second);
    check.expect(
        d.desktop == Status::Configured { port: SECOND_PORT },
        &format!("reconfigured, Desktop: {}", d.desktop.text()),
    );
    if code {
        check.expect(
            d.code == Status::Configured { port: SECOND_PORT },
            &format!("reconfigured, Claude Code: {}", d.code.text()),
        );
    }
    for (path, seed) in [(&classic, CLASSIC), (&store, STORE)] {
        check.expect(
            without_ours(path) == seed,
            &format!("{}: still every other byte", path.display()),
        );
    }

    // The app closed: Claude Code's own health check finds the bridge connected (it starts it
    // and lists its tools, as a session does).
    if code {
        let health = Command::new(cli.as_ref().unwrap())
            .args(["mcp", "get", clients::NAME])
            .env("CLAUDE_CONFIG_DIR", code_config.parent().unwrap())
            .current_dir(&locations.home)
            .stdin(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).replace(&token, "<token>"))
            .unwrap_or_default();
        let status = health
            .lines()
            .find(|l| l.trim_start().starts_with("Status:"))
            .unwrap_or("no status line")
            .trim()
            .to_string();
        check.expect(
            status.contains("Connected") && !status.contains("Failed"),
            &format!("Claude Code, the app closed: `claude mcp get oxide` says \"{status}\""),
        );
    }

    // Each client's bridge, started as its config says, answers by itself.
    let catalog = Catalog::embedded();
    let mut bridges = Vec::new();
    let mut configs = vec![("Desktop", store.clone())];
    if code {
        configs.push(("Claude Code", code_config.clone()));
    }
    for (name, config) in configs {
        match Bridge::start(name, &config) {
            Err(e) => check.fail(&format!("{name}: starting the bridge from its config: {e}")),
            Ok(bridge) => bridges.push(bridge),
        }
    }
    for b in &mut bridges {
        let name = b.name.clone();
        b.send(initialize(0));
        let init = b.answer(0, Duration::from_secs(30));
        let result = init
            .as_ref()
            .map(|m| m["result"].clone())
            .unwrap_or_default();
        check.expect(
            result["serverInfo"]["name"] == clients::NAME
                && result["instructions"]
                    .as_str()
                    .is_some_and(|i| i.ends_with(RUNNING_NOTE)),
            &format!(
                "{name}, the app closed: initialize answered by the bridge, saying Oxide must run"
            ),
        );
        b.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        b.send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
        let tools = b
            .answer(1, Duration::from_secs(30))
            .and_then(|m| m["result"]["tools"].as_array().cloned())
            .unwrap_or_default();
        check.expect(
            tools == catalog.tools && tools.iter().any(|t| t["name"] == "get_project"),
            &format!(
                "{name}, the app closed: tools/list has the registry's {} tools",
                tools.len()
            ),
        );
        b.send(json!({"jsonrpc": "2.0", "id": 2, "method": "prompts/list"}));
        let prompts = b
            .answer(2, Duration::from_secs(30))
            .and_then(|m| m["result"]["prompts"].as_array().cloned())
            .unwrap_or_default();
        check.expect(
            prompts == catalog.prompts,
            &format!(
                "{name}, the app closed: prompts/list has {} prompt(s)",
                prompts.len()
            ),
        );
        let called = b.get_project(3);
        check.expect(
            called == Some((NOT_RUNNING.to_string(), true)),
            &format!(
                "{name}, the app closed: a call is the tool error \"{NOT_RUNNING}\" ({called:?})"
            ),
        );
    }

    // The app starts on the new port: the same bridges reach it, nothing restarted.
    let mut daw = Command::new(app)
        .args([
            "--demo",
            audio,
            "--mute",
            "--mcp-port",
            &SECOND_PORT.to_string(),
            "--mcp-token",
            &token,
            "--exit-after",
            "120",
        ])
        .arg("--claude-profile")
        .arg(root.join("app"))
        .spawn()
        .expect("starting the app");
    let start = Instant::now();
    while !port_open(SECOND_PORT) && start.elapsed() < Duration::from_secs(90) {
        std::thread::sleep(Duration::from_millis(200));
    }
    if check.expect(
        port_open(SECOND_PORT),
        &format!(
            "the app serves on {SECOND_PORT} ({:.1} s)",
            start.elapsed().as_secs_f64()
        ),
    ) {
        for b in &mut bridges {
            let name = b.name.clone();
            let called = b.get_project(4);
            let through = called
                .as_ref()
                .is_some_and(|(text, error)| !error && text.contains("\"tracks\""));
            check.expect(
                through,
                &format!(
                    "{name}: with the app started, get_project goes through the same bridge ({} bytes)",
                    called.map_or(0, |(text, _)| text.len())
                ),
            );
            b.send(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/list"}));
            let tools = b
                .answer(5, Duration::from_secs(30))
                .and_then(|m| m["result"]["tools"].as_array().cloned())
                .unwrap_or_default();
            let changed = b
                .notes
                .iter()
                .any(|m| m["method"] == "notifications/tools/list_changed");
            check.expect(
                tools == catalog.tools && !changed,
                &format!(
                    "{name}: the app serves the bridge's {} tools (no list_changed)",
                    tools.len()
                ),
            );
        }
    }
    let _ = daw.kill();
    let _ = daw.wait();
    let start = Instant::now();
    while port_open(SECOND_PORT) && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(100));
    }
    // The app closed again: the calls say so.
    for mut b in bridges {
        let name = b.name.clone();
        let called = b.get_project(6);
        check.expect(
            called == Some((NOT_RUNNING.to_string(), true)),
            &format!("{name}: the app closed again, a call says \"{NOT_RUNNING}\""),
        );
        check.expect(
            b.finish(),
            &format!("{name}: the bridge ends when its stdin closes"),
        );
    }

    // Remove: the files as they were.
    if let Err(e) = clients::apply(&locations, Client::Desktop, Act::Remove, &second) {
        check.fail(&format!("Desktop: remove: {e}"));
    }
    for (path, seed) in [(&classic, CLASSIC), (&store, STORE)] {
        check.expect(
            read(path) == seed,
            &format!("{} as it was after Remove", path.display()),
        );
    }
    if code {
        if let Err(e) = clients::apply(&locations, Client::Code, Act::Remove, &second) {
            check.fail(&format!("Claude Code: remove: {e}"));
        }
        let value = json_edit::parse(&read(&code_config)).unwrap_or_default();
        check.expect(
            value["mcpServers"].get(clients::NAME).is_none()
                && value["mcpServers"]["rea"]["command"] == "rea.exe",
            "Claude Code: oxide removed, its other server kept",
        );

        // An entry over HTTP, as Connect Claude added it before 23.3 (through the real CLI):
        // read as such, and Reconfigure moves it to the bridge.
        let added = Command::new(cli.as_ref().unwrap())
            .args([
                "mcp",
                "add",
                "--transport",
                "http",
                "--scope",
                "user",
                clients::NAME,
            ])
            .arg(second.url())
            .env("CLAUDE_CONFIG_DIR", code_config.parent().unwrap())
            .current_dir(&locations.home)
            .stdin(Stdio::null())
            .output();
        check.expect(
            added.as_ref().is_ok_and(|o| o.status.success()),
            "Claude Code: an HTTP entry added by the CLI",
        );
        let d = clients::detect(&locations, &second);
        check.expect(
            d.code
                == Status::OverHttp {
                    port: Some(SECOND_PORT),
                },
            &format!("Claude Code over HTTP: {}", d.code.text()),
        );
        match clients::apply(&locations, Client::Code, Act::Reconfigure, &second) {
            Ok(said) => check.pass(&format!("Claude Code reconfigured from HTTP: {said}")),
            Err(e) => check.fail(&format!("Claude Code reconfigured from HTTP: {e}")),
        }
        let value = json_edit::parse(&read(&code_config)).unwrap_or_default();
        let entry = &value["mcpServers"][clients::NAME];
        check.expect(
            clients::detect(&locations, &second).code == Status::Configured { port: SECOND_PORT }
                && entry.get("url").is_none()
                && entry["command"] == locations.bridge.to_string_lossy().as_ref(),
            "Claude Code moved from HTTP to the bridge",
        );
        if let Err(e) = clients::apply(&locations, Client::Code, Act::Remove, &second) {
            check.fail(&format!("Claude Code: remove again: {e}"));
        }
    }
    let d = clients::detect(&locations, &second);
    check.expect(
        d.desktop == Status::NotConfigured && (!code || d.code == Status::NotConfigured),
        "both not configured after Remove",
    );

    // The user's own configs: untouched.
    let after = users_own();
    for ((path, was), (_, now)) in before.iter().zip(&after) {
        check.expect(was == now, &format!("the user's {path} untouched"));
    }
    check.expect(
        before.len() == after.len(),
        "the user's Claude configs: the same files",
    );

    if check.failed {
        std::process::exit(1);
    }
    // Claude Code's folder holds what the CLI copied of the account: not left lying about.
    let _ = std::fs::remove_dir_all(&root);
}
