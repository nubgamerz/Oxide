//! The stdio bridge Claude runs (`oxide-mcp.exe --port N [--token T]`): Claude Desktop always,
//! and Claude Code since 23.3.
//!
//! Desktop's config only starts local programs that speak MCP on their stdin and stdout, one
//! JSON-RPC message a line; Oxide serves Streamable HTTP. The bridge forwards each line as a
//! POST to `http://127.0.0.1:N/mcp` (with the bearer token, the session id and the protocol
//! version) and writes back what comes in reply, whether plain JSON or an event stream. It
//! needs no Node (`mcp-remote`) and writes nothing to disk: diagnostics go to stderr, which
//! the client keeps in its own log.
//!
//! Oxide comes and goes while Claude stays up, and a client that can't list a server's tools
//! marks the server failed and never asks again (Desktop did, 23.3). So the bridge never needs
//! the app to be loaded:
//! - `initialize`, `tools/list`, `prompts/list` and `ping` are answered here, from the
//!   [`Catalog`] the bridge was built with (the registry's tools and prompts, made by daw-core's
//!   `docs_are_current`), at once and whether or not Oxide runs. The instructions say Oxide must
//!   be running for the tools to act.
//! - `tools/call` opens a session with the app when it has none (so Oxide started after
//!   Claude is reached by the next call, with nothing restarted) and relays the call. While
//!   Oxide is closed the call is answered with a tool error ([`NOT_RUNNING`], `isError`), not
//!   a protocol error, which clients take for a broken server.
//! - Once a session is open, the app's own tool list is fetched; when it differs from the one
//!   Claude was given (another version of Oxide), Claude is sent `tools/list_changed` and
//!   later lists get the app's, until the app goes away again. A call to a tool the app
//!   doesn't have but the bridge listed says which versions differ.
//! - A watcher looks for the app every 2 s while there is no session, so a differing list
//!   reaches Claude without waiting for a call.
//! - A session the app no longer knows (it restarted: 404) is opened again and the request
//!   sent once more.
//!
//! Requests run on threads of their own (a ping or a cancel isn't stuck behind a tool call);
//! notifications go in order, on the reading thread, and are dropped while there is no
//! session (never a connect attempt for one, which takes 2 s on Windows when nothing listens).

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What a call gets while the app is closed.
pub const NOT_RUNNING: &str = "Oxide isn't running: start it, then try again";

/// Added to the catalog's instructions in the bridge's `initialize`.
pub const RUNNING_NOTE: &str = "Oxide (the DAW) must be running for these tools to act: while \
     it is closed every call answers that it isn't running. Ask the user to start Oxide, then \
     call again; nothing needs restarting.";

/// What a request gets when the app turns the bridge's token away.
const TOKEN_REFUSED: &str = "Oxide refused the bridge's token: in Oxide, Options > AI control \
     (MCP), Reconfigure Claude Code and Claude Desktop (then restart Desktop)";

/// The JSON-RPC error code for the bridge's own errors (the "server error" range).
const ERROR: i64 = -32000;

/// How long a reply may take: the app's own call timeout is 300 s, and an open waits for it.
const REPLY_LIMIT: Duration = Duration::from_secs(330);

/// How long connecting may take (loopback: refused at once when nothing listens, though
/// Windows retries a refused connect for about 2 s).
const CONNECT_LIMIT: Duration = Duration::from_secs(2);

/// How often the watcher looks for the app while there is no session.
const WATCH_EVERY: Duration = Duration::from_secs(2);

/// The protocol versions with an `initialize` the bridge answers in (the app's rmcp knows
/// them all); a client asking for another gets the last.
const VERSIONS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// The tools, prompts and instructions the bridge answers with by itself: the registry it
/// was built from (`catalog.json`, kept current by daw-core's `docs_are_current`).
#[derive(Clone, Debug, PartialEq)]
pub struct Catalog {
    pub instructions: String,
    pub tools: Vec<Value>,
    pub prompts: Vec<Value>,
}

impl Catalog {
    /// A catalog in `catalog.json`'s form.
    pub fn parse(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let list = |name: &str| {
            value[name]
                .as_array()
                .cloned()
                .ok_or_else(|| format!("the catalog has no {name} list"))
        };
        Ok(Self {
            instructions: value["instructions"].as_str().unwrap_or("").to_string(),
            tools: list("tools")?,
            prompts: list("prompts")?,
        })
    }

    /// The one built into the bridge.
    pub fn embedded() -> Self {
        Self::parse(include_str!("catalog.json"))
            .expect("catalog.json is written by daw-core's docs_are_current")
    }

    fn has_tool(tools: &[Value], name: &str) -> bool {
        tools.iter().any(|t| t["name"] == name)
    }
}

/// The bridge's arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub port: u16,
    pub token: Option<String>,
}

pub const USAGE: &str = "oxide-mcp --port N [--token TOKEN]\n\nConnects an MCP client that \
    speaks stdio (Claude Desktop, Claude Code) to Oxide's MCP server on 127.0.0.1:N, and lists \
    Oxide's tools even while Oxide is closed. Oxide's settings (Options > AI control (MCP), \
    Connect Claude) write this into Claude's configs.";

/// Read `--port N` and `--token T` (the program name already taken off).
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut port = None;
    let mut token = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                let value = args.next().ok_or("--port needs a number")?;
                port = Some(
                    value
                        .parse::<u16>()
                        .ok()
                        .filter(|p| *p > 0)
                        .ok_or_else(|| format!("--port {value}: not a port"))?,
                );
            }
            "--token" => token = Some(args.next().ok_or("--token needs a value")?),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(Options {
        port: port.ok_or("--port is required")?,
        token,
    })
}

/// Why a request didn't get an HTTP reply.
enum Failed {
    /// Nothing listens on the port: the app is closed.
    Unreachable,
    /// The connection broke or the reply couldn't be read.
    Broken(String),
    /// Something that isn't an HTTP server answers on the port (the whole message).
    Stranger(String),
}

impl Failed {
    /// What a client is told, when the app isn't simply closed.
    fn why(self) -> String {
        match self {
            Self::Unreachable => NOT_RUNNING.into(),
            Self::Broken(e) => format!("Oxide's reply broke off: {e}"),
            Self::Stranger(why) => why,
        }
    }
}

/// An HTTP reply, its body still to be read.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Box<dyn BufRead + Send>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn text(mut self) -> String {
        let mut text = String::new();
        let _ = self.body.read_to_string(&mut text);
        text
    }
}

/// A body sent in `Transfer-Encoding: chunked`, read as the plain bytes.
struct Chunked<R> {
    inner: R,
    left: usize,
    done: bool,
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let mut line = String::new();
            self.inner.read_line(&mut line)?;
            let size = line.trim().split(';').next().unwrap_or("");
            let size = usize::from_str_radix(size.trim(), 16).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "a bad chunk size")
            })?;
            if size == 0 {
                // Trailers, then the empty line.
                loop {
                    line.clear();
                    if self.inner.read_line(&mut line)? == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                self.done = true;
                return Ok(0);
            }
            self.left = size;
        }
        let want = buf.len().min(self.left);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        self.left -= n;
        if self.left == 0 {
            let mut crlf = [0u8; 2];
            self.inner.read_exact(&mut crlf)?;
        }
        Ok(n)
    }
}

/// A connection to `port` on loopback, or `None` when nothing listens there.
///
/// A connect to a port nobody listens on can meet itself: when the port is in the ephemeral
/// range and the system picks the same one as the connection's own end, the two ends are one
/// socket (TCP's simultaneous open) and what the bridge writes comes back as the "reply". Such
/// a connection is nothing listening.
fn connect(port: u16) -> Option<TcpStream> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let stream = TcpStream::connect_timeout(&address, CONNECT_LIMIT).ok()?;
    let itself = matches!(
        (stream.local_addr(), stream.peer_addr()),
        (Ok(local), Ok(peer)) if local == peer
    );
    (!itself).then_some(stream)
}

/// One HTTP/1.1 request to the app on loopback, a connection of its own.
fn request(
    port: u16,
    method: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<Reply, Failed> {
    let mut stream = connect(port).ok_or(Failed::Unreachable)?;
    let broken = |e: std::io::Error| Failed::Broken(e.to_string());
    stream.set_read_timeout(Some(REPLY_LIMIT)).map_err(broken)?;
    stream.set_nodelay(true).map_err(broken)?;
    let mut head = format!(
        "{method} /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\n\
         Connection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).map_err(broken)?;
    stream.write_all(body).map_err(broken)?;
    stream.flush().map_err(broken)?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(broken)?;
    let status = line
        .starts_with("HTTP/")
        .then(|| line.split_whitespace().nth(1)?.parse().ok())
        .flatten()
        .ok_or_else(|| {
            let said: String = line.trim().chars().take(80).collect();
            Failed::Stranger(format!(
                "something other than Oxide's MCP server answers on port {port} (it said \
                 {said:?}): check the port in Oxide's Options > AI control (MCP), then \
                 Reconfigure"
            ))
        })?;
    let mut headers = Vec::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(broken)? == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    let find = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v): &(String, String)| v.clone())
    };
    let body: Box<dyn BufRead + Send> =
        if find("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
            Box::new(BufReader::new(Chunked {
                inner: reader,
                left: 0,
                done: false,
            }))
        } else if let Some(length) = find("content-length").and_then(|v| v.parse::<u64>().ok()) {
            Box::new(reader.take(length))
        } else {
            Box::new(reader)
        };
    Ok(Reply {
        status,
        headers,
        body,
    })
}

/// The JSON-RPC messages in an event stream, handed to `each` as they arrive; `each` returns
/// false to stop reading (the reply it waited for came).
fn events(body: &mut dyn BufRead, mut each: impl FnMut(Value) -> bool) {
    let mut data = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let end = !matches!(body.read_line(&mut line), Ok(n) if n > 0);
        let text = line.trim_end_matches(['\r', '\n']);
        if end || text.is_empty() {
            if !data.is_empty() {
                if let Ok(message) = serde_json::from_str::<Value>(&data) {
                    if !each(message) {
                        return;
                    }
                }
                data.clear();
            }
            if end {
                return;
            }
        } else if let Some(value) = text.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
        // Comments (keep-alives), `event:`, `id:` and `retry:` lines carry nothing to relay.
    }
}

/// The running app, as a session with it found it.
struct App {
    /// Its version (`serverInfo.version`), if it said.
    version: Option<String>,
    /// Its `tools/list`.
    tools: Vec<Value>,
}

/// The MCP session the bridge holds with the app.
#[derive(Default)]
struct Upstream {
    /// `Mcp-Session-Id`, once the app has answered an `initialize`.
    session: Option<String>,
    /// The protocol version it agreed.
    version: Option<String>,
    /// The client's own `initialize` params (with the version the bridge answered in), sent to
    /// open a session.
    init: Option<Value>,
    /// The app, while a session is open and its tools were listed.
    app: Option<App>,
}

/// The bridge: what arrives on stdin goes to [`Bridge::handle`]; replies are written to `out`.
pub struct Bridge<W> {
    options: Options,
    catalog: Catalog,
    /// The Oxide version the bridge comes from (for the version note).
    version: String,
    out: Mutex<W>,
    upstream: Mutex<Upstream>,
    /// Held while a session is (re)opened, so two requests don't open two.
    opening: Mutex<()>,
    watching: AtomicBool,
    closed: AtomicBool,
}

/// What forwarding one message came to.
enum Sent {
    /// Relayed (or nothing to relay).
    Done,
    /// The app doesn't know the session.
    Expired,
    Failed(String),
}

impl<W: Write + Send + 'static> Bridge<W> {
    /// A bridge with the catalog built into it.
    pub fn new(options: Options, out: W) -> Arc<Self> {
        Self::with(options, Catalog::embedded(), env!("CARGO_PKG_VERSION"), out)
    }

    /// A bridge answering from `catalog`, from Oxide `version`.
    pub fn with(options: Options, catalog: Catalog, version: &str, out: W) -> Arc<Self> {
        Arc::new(Self {
            options,
            catalog,
            version: version.to_string(),
            out: Mutex::new(out),
            upstream: Mutex::default(),
            opening: Mutex::new(()),
            watching: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        })
    }

    fn upstream(&self) -> std::sync::MutexGuard<'_, Upstream> {
        self.upstream.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Write one message to the client: a line of compact JSON.
    fn send(&self, message: &Value) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(out, "{message}");
        let _ = out.flush();
    }

    fn reply(&self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn error(&self, id: &Value, message: &str) {
        self.send(
            &json!({"jsonrpc": "2.0", "id": id, "error": {"code": ERROR, "message": message}}),
        );
    }

    /// A tool's failure as MCP has it: a result the agent reads, not a broken server.
    fn tool_error(&self, id: &Value, text: &str) {
        self.reply(
            id,
            json!({"content": [{"type": "text", "text": text}], "isError": true}),
        );
    }

    fn headers(&self, upstream: &Upstream) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            (
                "Accept".to_string(),
                "application/json, text/event-stream".to_string(),
            ),
        ];
        if let Some(token) = &self.options.token {
            headers.push(("Authorization".into(), format!("Bearer {token}")));
        }
        if let Some(session) = &upstream.session {
            headers.push(("Mcp-Session-Id".into(), session.clone()));
        }
        if let Some(version) = &upstream.version {
            headers.push(("MCP-Protocol-Version".into(), version.clone()));
        }
        headers
    }

    fn post(&self, message: &Value) -> Result<Reply, Failed> {
        let headers = self.headers(&self.upstream());
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        request(
            self.options.port,
            "POST",
            &headers,
            message.to_string().as_bytes(),
        )
    }

    /// The messages in a reply, in order: the plain JSON one (or a batch's array), or each
    /// event's. Stops at the response to `id` when there is one to wait for.
    fn messages(reply: Reply, id: Option<&Value>, mut each: impl FnMut(Value)) {
        let event_stream = reply
            .header("content-type")
            .is_some_and(|t| t.starts_with("text/event-stream"));
        if event_stream {
            let mut body = reply.body;
            events(&mut body, |message| {
                let answers = id.is_some_and(|id| {
                    message.get("id") == Some(id) && message.get("method").is_none()
                });
                each(message);
                !answers
            });
        } else {
            let text = reply.text();
            if !text.trim().is_empty() {
                if let Ok(message) = serde_json::from_str::<Value>(&text) {
                    each(message);
                }
            }
        }
    }

    /// The tools Claude gets from `tools/list` now: the app's while a session knows them, else
    /// the catalog's.
    fn listed<'a>(&'a self, upstream: &'a Upstream) -> &'a [Value] {
        upstream
            .app
            .as_ref()
            .map_or(&self.catalog.tools[..], |app| &app.tools[..])
    }

    /// The app found (or lost): tell the client the tools changed if what it lists does.
    fn set_app(&self, app: Option<App>) {
        let changed = {
            let mut upstream = self.upstream();
            let before = self.listed(&upstream).to_vec();
            upstream.app = app;
            before != self.listed(&upstream)
        };
        if changed && !self.closed.load(Ordering::SeqCst) {
            self.send(&json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}));
        }
    }

    /// The app is gone: forget the session, list the catalog again and watch for it.
    fn lost(self: &Arc<Self>) {
        {
            let mut upstream = self.upstream();
            upstream.session = None;
            upstream.version = None;
        }
        self.set_app(None);
        self.watch();
    }

    /// Send `message` on the session the bridge holds and relay what comes back.
    fn forward(self: &Arc<Self>, message: &Value) -> Sent {
        let reply = match self.post(message) {
            Ok(reply) => reply,
            Err(Failed::Unreachable) => {
                self.lost();
                return Sent::Failed(NOT_RUNNING.into());
            }
            Err(failed) => return Sent::Failed(failed.why()),
        };
        match reply.status {
            200..=299 => {
                Self::messages(reply, message.get("id"), |m| self.send(&m));
                Sent::Done
            }
            404 if self.upstream().session.is_some() => Sent::Expired,
            401 => Sent::Failed(TOKEN_REFUSED.into()),
            status => {
                let text = reply.text();
                let text: String = text.trim().chars().take(300).collect();
                Sent::Failed(format!("Oxide answered HTTP {status}: {text}"))
            }
        }
    }

    /// The one message answering `id` in a reply to the bridge's own request.
    fn answer_to(reply: Reply, id: &Value) -> Option<Value> {
        let mut answer = None;
        Self::messages(reply, Some(id), |m| {
            if m.get("id") == Some(id) {
                answer = Some(m);
            }
        });
        answer
    }

    /// Open a session with the app using the client's `initialize` (sent with the bridge's own
    /// id), followed by `notifications/initialized`, and list the app's tools. Why not, if the
    /// app isn't there or refuses.
    fn open_session(&self) -> Result<(), String> {
        let _opening = self.opening.lock().unwrap_or_else(|e| e.into_inner());
        let params = {
            let upstream = self.upstream();
            if upstream.session.is_some() {
                return Ok(());
            }
            upstream
                .init
                .clone()
                .ok_or("the client hasn't sent initialize")?
        };
        let id = json!("oxide-mcp-bridge");
        let init = json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": params});
        let reply = match self.post(&init) {
            Ok(reply) => reply,
            Err(failed) => return Err(failed.why()),
        };
        match reply.status {
            200 => {}
            401 => return Err(TOKEN_REFUSED.into()),
            status => {
                let text: String = reply.text().trim().chars().take(300).collect();
                return Err(format!("Oxide answered HTTP {status}: {text}"));
            }
        }
        let session = reply.header("mcp-session-id").map(str::to_string);
        let answer = Self::answer_to(reply, &id);
        let result = answer.as_ref().and_then(|m| m.get("result"));
        let version = result
            .and_then(|r| r["protocolVersion"].as_str())
            .map(str::to_string);
        if session.is_none() && version.is_none() {
            return Err("Oxide didn't answer initialize".into());
        }
        let app_version = result
            .and_then(|r| r.pointer("/serverInfo/version"))
            .and_then(Value::as_str)
            .map(str::to_string);
        {
            let mut upstream = self.upstream();
            upstream.session = session;
            upstream.version = version;
        }
        let _ = self.post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        // The app's tools, to compare with what Claude was given.
        let id = json!("oxide-mcp-bridge-tools");
        let tools = self
            .post(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"}))
            .ok()
            .filter(|r| r.status == 200)
            .and_then(|r| Self::answer_to(r, &id))
            .and_then(|m| {
                m.pointer("/result/tools")
                    .and_then(Value::as_array)
                    .cloned()
            });
        if let Some(tools) = tools {
            self.set_app(Some(App {
                version: app_version,
                tools,
            }));
        }
        Ok(())
    }

    /// The client's `initialize`, answered here from the catalog whether or not the app runs.
    fn initialize(self: &Arc<Self>, id: &Value, message: &Value) {
        let mut params = message.get("params").cloned().unwrap_or(json!({}));
        let asked = params["protocolVersion"].as_str().unwrap_or("");
        let version = VERSIONS
            .iter()
            .find(|v| **v == asked)
            .unwrap_or(&VERSIONS[VERSIONS.len() - 1])
            .to_string();
        if let Some(params) = params.as_object_mut() {
            // The app is asked for the version the client was told.
            params.insert("protocolVersion".into(), json!(version));
        }
        *self.upstream() = Upstream {
            init: Some(params),
            ..Upstream::default()
        };
        let instructions = if self.catalog.instructions.is_empty() {
            RUNNING_NOTE.to_string()
        } else {
            format!("{} {RUNNING_NOTE}", self.catalog.instructions)
        };
        self.reply(
            id,
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": true}, "prompts": {}},
                "serverInfo": {"name": "oxide", "title": "Oxide", "version": self.version},
                "instructions": instructions,
            }),
        );
        self.watch();
    }

    /// While there is no session: look for the app every [`WATCH_EVERY`], and open one when
    /// its port answers, so a tool list that differs reaches the client without a call.
    fn watch(self: &Arc<Self>) {
        if self.watching.swap(true, Ordering::SeqCst) {
            return;
        }
        let bridge = Arc::clone(self);
        let _ = std::thread::Builder::new()
            .name("oxide-mcp-watch".into())
            .spawn(move || {
                while !bridge.closed.load(Ordering::SeqCst) && bridge.upstream().session.is_none() {
                    if connect(bridge.options.port).is_some() {
                        match bridge.open_session() {
                            Ok(()) => break,
                            // The app is there but refuses (a token): calls will say so.
                            Err(why) if why != NOT_RUNNING => {
                                eprintln!("oxide-mcp: {why}");
                                break;
                            }
                            Err(_) => {}
                        }
                    }
                    std::thread::sleep(WATCH_EVERY);
                }
                bridge.watching.store(false, Ordering::SeqCst);
            });
    }

    /// Relay a request on the session, opening one if need be; why not, if it couldn't be.
    fn relay(self: &Arc<Self>, message: &Value) -> Result<(), String> {
        for _ in 0..2 {
            self.open_session()?;
            match self.forward(message) {
                Sent::Done => return Ok(()),
                // The app restarted: open a new session and send it again.
                Sent::Expired => {
                    let mut upstream = self.upstream();
                    upstream.session = None;
                    upstream.version = None;
                }
                Sent::Failed(why) => return Err(why),
            }
        }
        Err("Oxide forgot the session twice in a row".into())
    }

    /// Why the app can't run tool `name` though the bridge listed it, if so: they are from
    /// different versions of Oxide.
    fn missing_tool(&self, name: &str) -> Option<String> {
        let upstream = self.upstream();
        let app = upstream.app.as_ref()?;
        if Catalog::has_tool(&app.tools, name) || !Catalog::has_tool(&self.catalog.tools, name) {
            return None;
        }
        let running = app
            .version
            .as_deref()
            .map_or("The running Oxide".to_string(), |v| format!("Oxide {v}"));
        Some(format!(
            "{running} has no tool named \"{name}\": it is another version than the bridge \
             Claude runs (from Oxide {}), which listed it. List the tools again, or bring them \
             in line: in Oxide, Options > AI control (MCP), Reconfigure Claude Code and Claude \
             Desktop, then start a new session",
            self.version
        ))
    }

    /// `tools/call`: relayed to the app, a tool error when it can't be.
    fn call_tool(self: &Arc<Self>, id: &Value, message: &Value) {
        if let Err(why) = self.open_session() {
            return self.tool_error(id, &why);
        }
        let name = message.pointer("/params/name").and_then(Value::as_str);
        if let Some(why) = name.and_then(|n| self.missing_tool(n)) {
            return self.tool_error(id, &why);
        }
        if let Err(why) = self.relay(message) {
            self.tool_error(id, &why);
        }
    }

    /// One line from the client.
    pub fn handle(self: &Arc<Self>, line: &str) {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(e) => {
                self.send(&json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}}));
                return;
            }
        };
        let method = message.get("method").and_then(Value::as_str);
        match (method, message.get("id")) {
            (Some("initialize"), Some(id)) => self.initialize(id, &message),
            (Some("ping"), Some(id)) => self.reply(id, json!({})),
            (Some("tools/list"), Some(id)) => {
                let tools = self.listed(&self.upstream()).to_vec();
                self.reply(id, json!({"tools": tools}));
            }
            (Some("prompts/list"), Some(id)) => {
                self.reply(id, json!({"prompts": self.catalog.prompts}));
            }
            (Some("tools/call"), Some(id)) => self.call_tool(id, &message),
            // A prompt's text is made by the app.
            (Some("prompts/get"), Some(id)) => {
                if let Err(why) = self.relay(&message) {
                    self.error(id, &why);
                }
            }
            // Anything else asked for: the app's to answer while a session is open. Without one,
            // what the app says to a method it doesn't serve (it declares only tools and
            // prompts), never "isn't running", which a client could take for a broken server.
            (Some(method), Some(id)) => {
                let open = self.upstream().session.is_some();
                if !open {
                    self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("Method not found: {method}")}}));
                } else if let Err(why) = self.relay(&message) {
                    self.error(id, &why);
                }
            }
            // The client's `initialized` was for the bridge, which sends its own to the app.
            (Some("notifications/initialized"), None) => {}
            // Other notifications, and the client's replies: only to a session that exists.
            _ => {
                if self.upstream().session.is_some() {
                    let _ = self.forward(&message);
                }
            }
        }
    }

    /// The client went away: stop watching and end the app's session.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let upstream = self.upstream();
        if upstream.session.is_some() {
            let headers = self.headers(&upstream);
            drop(upstream);
            let headers: Vec<(&str, &str)> = headers
                .iter()
                .map(|(n, v)| (n.as_str(), v.as_str()))
                .collect();
            let _ = request(self.options.port, "DELETE", &headers, b"");
        }
    }
}

/// Read the client's lines until it closes stdin: `initialize` and notifications in order on
/// this thread, other requests each on a thread of their own. Returns once every reply is out.
pub fn run<W: Write + Send + 'static>(bridge: &Arc<Bridge<W>>, input: impl BufRead) {
    let mut running: Vec<std::thread::JoinHandle<()>> = Vec::new();
    for line in input.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let in_order = serde_json::from_str::<Value>(line).map_or(true, |m| {
            m.get("id").is_none() || m.get("method").and_then(Value::as_str) == Some("initialize")
        });
        if in_order {
            bridge.handle(line);
        } else {
            let bridge = Arc::clone(bridge);
            let line = line.to_string();
            match std::thread::Builder::new()
                .name("oxide-mcp-call".into())
                .spawn(move || bridge.handle(&line))
            {
                Ok(thread) => running.push(thread),
                Err(e) => eprintln!("oxide-mcp: no thread for a request: {e}"),
            }
        }
        running.retain(|t| !t.is_finished());
    }
    for thread in running {
        let _ = thread.join();
    }
    bridge.close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    #[test]
    fn arguments() {
        let args = |a: &[&str]| parse_args(a.iter().map(|s| s.to_string()));
        assert_eq!(
            args(&["--port", "7878"]),
            Ok(Options {
                port: 7878,
                token: None
            })
        );
        assert_eq!(
            args(&["--token", "ab", "--port", "1024"]),
            Ok(Options {
                port: 1024,
                token: Some("ab".into())
            })
        );
        assert!(args(&[]).unwrap_err().contains("--port is required"));
        assert!(args(&["--port", "0"]).is_err());
        assert!(args(&["--port", "x"]).is_err());
        assert!(args(&["--port"]).is_err());
        assert!(args(&["--verbose"]).unwrap_err().contains("--verbose"));
    }

    #[test]
    fn chunked_bodies_and_event_streams_are_read_as_sent() {
        let raw = b"7\r\ndata: {\r\n14\r\n\"id\":1,\"a\":2}\n\n: ka\n\r\n0\r\n\r\n";
        let mut body = BufReader::new(Chunked {
            inner: &raw[..],
            left: 0,
            done: false,
        });
        let mut text = String::new();
        body.read_to_string(&mut text).unwrap();
        assert_eq!(text, "data: {\"id\":1,\"a\":2}\n\n: ka\n");

        let stream = "id: 0\ndata:\n\n: keep-alive\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"n\"}\n\ndata: {\"jsonrpc\":\"2.0\",\r\ndata: \"id\":3,\"result\":{}}\r\n\r\ndata: {\"never\":1}\n\n";
        let mut seen = Vec::new();
        events(&mut stream.as_bytes(), |m| {
            let done = m.get("id") == Some(&json!(3));
            seen.push(m);
            !done
        });
        assert_eq!(
            seen,
            vec![
                json!({"jsonrpc": "2.0", "method": "n"}),
                json!({"jsonrpc": "2.0", "id": 3, "result": {}})
            ]
        );
    }

    /// A shared buffer the bridge writes to.
    #[derive(Clone, Default)]
    pub struct Lines(Arc<Mutex<Vec<u8>>>);

    impl Write for Lines {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Lines {
        fn take(&self) -> Vec<Value> {
            let bytes = std::mem::take(&mut *self.0.lock().unwrap());
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
    }

    /// A loopback port nothing listens on, for one test alone. Not one the system hands out
    /// (`bind(0)`, the ephemeral range from 49152), so no other test's `bind(0)` or outgoing
    /// connection can take it while this one expects it quiet or binds its stand-in there:
    /// ports from 21000 by process (tests of other runs) and then a counter (tests running
    /// beside each other in this one), each checked free (Hyper-V's reserved ranges skipped).
    fn free_port() -> u16 {
        static NEXT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
        let base = 21_000 + (std::process::id() % 500) as u16 * 16;
        loop {
            let port = base + NEXT.fetch_add(1, Ordering::SeqCst) % 8_000;
            if TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return port;
            }
        }
    }

    const INIT: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;

    fn call(id: i64, name: &str) -> String {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": {}}}).to_string()
    }

    fn tools_changed(lines: &[Value]) -> usize {
        lines
            .iter()
            .filter(|m| m["method"] == "notifications/tools/list_changed")
            .count()
    }

    fn answer(lines: &[Value], id: i64) -> &Value {
        lines
            .iter()
            .find(|m| m["id"] == id)
            .unwrap_or_else(|| panic!("no answer to {id}: {lines:?}"))
    }

    #[test]
    fn the_embedded_catalog_has_the_registrys_tools_and_prompts() {
        let catalog = Catalog::embedded();
        assert!(catalog.tools.len() > 100, "{} tools", catalog.tools.len());
        for name in ["get_project", "add_track", "screenshot", "get_busy"] {
            assert!(Catalog::has_tool(&catalog.tools, name), "{name}");
        }
        assert!(catalog.tools.iter().all(|t| t["name"].is_string()
            && t["description"].is_string()
            && t["inputSchema"].is_object()));
        assert!(catalog
            .prompts
            .iter()
            .any(|p| p["name"] == "write_midi_part"));
        assert!(catalog.instructions.contains("get_project"));
    }

    #[test]
    fn with_the_app_closed_the_bridge_lists_its_tools_and_calls_say_why() {
        let port = free_port();
        let out = Lines::default();
        let bridge = Bridge::new(Options { port, token: None }, out.clone());
        let catalog = Catalog::embedded();
        let input = [
            INIT,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"prompts/list"}"#,
            &call(4, "get_project"),
            r#"{"jsonrpc":"2.0","id":5,"method":"prompts/get","params":{"name":"write_midi_part"}}"#,
            r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#,
            "not json",
        ]
        .join("\n");
        run(&bridge, input.as_bytes());
        let lines = out.take();
        assert_eq!(lines.len(), 8, "{lines:?}");
        let parse = lines.iter().find(|m| m["id"].is_null()).unwrap();
        assert_eq!(parse["error"]["code"], -32700);
        let init = &answer(&lines, 0)["result"];
        assert_eq!(init["serverInfo"]["name"], "oxide");
        assert_eq!(init["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(init["protocolVersion"], "2025-06-18");
        assert_eq!(init["capabilities"]["tools"]["listChanged"], true);
        let instructions = init["instructions"].as_str().unwrap();
        assert!(
            instructions.starts_with(&catalog.instructions),
            "{instructions}"
        );
        assert!(instructions.ends_with(RUNNING_NOTE), "{instructions}");
        // The registry's tools and prompts, though nothing listens.
        assert_eq!(answer(&lines, 1)["result"]["tools"], json!(catalog.tools));
        assert_eq!(answer(&lines, 2)["result"], json!({}));
        assert_eq!(
            answer(&lines, 3)["result"]["prompts"],
            json!(catalog.prompts)
        );
        // A call is a tool error the agent reads, not a protocol error.
        let called = answer(&lines, 4);
        assert!(called.get("error").is_none(), "{called}");
        assert_eq!(called["result"]["isError"], true);
        assert_eq!(called["result"]["content"][0]["text"], NOT_RUNNING);
        // Prompt text is the app's to make.
        assert_eq!(answer(&lines, 5)["error"]["message"], NOT_RUNNING);
        // A method neither serves is "not found", as the app says it, not "isn't running".
        assert_eq!(answer(&lines, 6)["error"]["code"], -32601);
        assert_eq!(tools_changed(&lines), 0);
    }

    /// A program that isn't an HTTP server on the port (here one that writes back a request,
    /// as a connection that met itself would) is named as such, not taken for a broken reply.
    #[test]
    fn something_else_on_the_port_is_said_to_be_not_oxide() {
        let port = free_port();
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        // Every connection (the watcher's too) gets the same.
        let stranger = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.write_all(b"POST /mcp HTTP/1.1\r\nHost: x\r\n\r\n");
                // Read all that is sent until the bridge hangs up, so the close is never a
                // reset that could overtake the line written (one short read left data
                // unread under load, and Windows reset the connection).
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                while matches!(stream.read(&mut [0; 8192]), Ok(n) if n > 0) {}
            }
        });
        let out = Lines::default();
        let bridge = Bridge::new(Options { port, token: None }, out.clone());
        bridge.handle(INIT);
        bridge.handle(&call(1, "get_project"));
        bridge.close();
        stop.store(true, Ordering::SeqCst);
        stranger.join().unwrap();
        let lines = out.take();
        let reply = answer(&lines, 1);
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(reply["result"]["isError"], true);
        assert!(
            text.starts_with(&format!(
                "something other than Oxide's MCP server answers on port {port}"
            )),
            "{text}"
        );
    }

    #[test]
    fn a_client_asking_for_an_unknown_version_gets_the_latest_known() {
        let out = Lines::default();
        let bridge = Bridge::new(
            Options {
                port: free_port(),
                token: None,
            },
            out.clone(),
        );
        bridge.handle(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2099-01-01","capabilities":{}}}"#);
        bridge.close();
        assert_eq!(out.take()[0]["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(
            bridge.upstream().init.as_ref().unwrap()["protocolVersion"],
            "2025-11-25",
            "the app is asked for the same"
        );
    }

    #[test]
    fn notifications_with_no_app_are_dropped_at_once() {
        let out = Lines::default();
        let bridge = Bridge::new(
            Options {
                port: free_port(),
                token: None,
            },
            out.clone(),
        );
        bridge.handle(INIT);
        out.take();
        let start = Instant::now();
        bridge.handle(
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":4}}"#,
        );
        bridge.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        // No connect attempt (2 s on Windows with nothing listening).
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
        bridge.close();
        assert!(out.take().is_empty());
    }

    /// A stand-in for Oxide's MCP server: Streamable HTTP's plain-JSON replies, a session id,
    /// `tools` for `tools/list`, and every `tools/call` answered "called <name>".
    struct FakeApp {
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl FakeApp {
        fn start(port: u16, tools: Vec<Value>, version: &'static str) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let calls = Arc::new(Mutex::new(Vec::new()));
            let (stopping, called) = (Arc::clone(&stop), Arc::clone(&calls));
            let thread = std::thread::spawn(move || {
                while !stopping.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            // A probe that connects and goes (the watcher's) is no request.
                            let _ = Self::serve(stream, &tools, version, &called);
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(2)),
                    }
                }
            });
            Self {
                stop,
                thread: Some(thread),
                calls,
            }
        }

        fn serve(
            stream: TcpStream,
            tools: &[Value],
            version: &str,
            calls: &Mutex<Vec<String>>,
        ) -> std::io::Result<()> {
            stream.set_nonblocking(false)?;
            let mut reader = BufReader::new(stream.try_clone()?);
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                return Ok(());
            }
            let method = line.split_whitespace().next().unwrap_or("").to_string();
            let mut length = 0;
            loop {
                line.clear();
                if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            let message: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let id = message["id"].clone();
            let result = match message["method"].as_str() {
                _ if method != "POST" => None,
                Some("initialize") => Some(json!({
                    "protocolVersion": message["params"]["protocolVersion"],
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "oxide", "version": version}
                })),
                Some("tools/list") => Some(json!({"tools": tools})),
                Some("tools/call") => {
                    let name = message["params"]["name"].as_str().unwrap_or("").to_string();
                    calls.lock().unwrap().push(name.clone());
                    Some(json!({"content": [{"type": "text", "text": format!("called {name}")}]}))
                }
                _ => None,
            };
            let (status, body) = match result {
                Some(result) => (
                    "200 OK",
                    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
                ),
                None => ("202 Accepted", String::new()),
            };
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nMcp-Session-Id: s1\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        }
    }

    impl Drop for FakeApp {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn tool(name: &str) -> Value {
        json!({"name": name, "description": name, "inputSchema": {"type": "object"}})
    }

    /// Claude started before Oxide (23.3): the bridge lists its own tools and calls say Oxide
    /// isn't running; Oxide starts and the next call goes through on the same bridge; the app
    /// being another version (other tools) is told to the client and to a call of a tool only
    /// the bridge has; Oxide closes and calls say so again.
    #[test]
    fn an_app_started_afterwards_is_reached_without_restarting_the_bridge() {
        let port = free_port();
        let out = Lines::default();
        let catalog = Catalog {
            instructions: "Use a.".into(),
            tools: vec![tool("a"), tool("b")],
            prompts: vec![],
        };
        let bridge = Bridge::with(
            Options { port, token: None },
            catalog.clone(),
            "1.0.0",
            out.clone(),
        );
        bridge.handle(INIT);
        bridge.handle(&call(1, "a"));
        let lines = out.take();
        assert_eq!(
            answer(&lines, 1)["result"]["content"][0]["text"],
            NOT_RUNNING
        );

        // Oxide starts (another version: it has "c", not "b").
        let app = FakeApp::start(port, vec![tool("a"), tool("c")], "2.0.0");
        bridge.handle(&call(2, "a"));
        let lines = out.take();
        let called = answer(&lines, 2);
        assert_eq!(
            called["result"]["content"][0]["text"], "called a",
            "{lines:?}"
        );
        assert!(called["result"].get("isError").is_none());
        assert_eq!(tools_changed(&lines), 1, "{lines:?}");
        assert_eq!(*app.calls.lock().unwrap(), vec!["a".to_string()]);
        bridge.handle(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
        bridge.handle(&call(4, "b"));
        let lines = out.take();
        assert_eq!(
            answer(&lines, 3)["result"]["tools"],
            json!([tool("a"), tool("c")])
        );
        let why = answer(&lines, 4)["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(answer(&lines, 4)["result"]["isError"], true);
        assert!(
            why.contains("Oxide 2.0.0 has no tool named \"b\"") && why.contains("from Oxide 1.0.0"),
            "{why}"
        );
        assert_eq!(
            app.calls.lock().unwrap().len(),
            1,
            "\"b\" never reached the app"
        );
        // A tool neither knows is the app's to refuse.
        bridge.handle(&call(5, "c"));
        assert_eq!(
            answer(&out.take(), 5)["result"]["content"][0]["text"],
            "called c"
        );

        // Oxide closes: calls say so, and the bridge's own list is back.
        drop(app);
        bridge.handle(&call(6, "a"));
        bridge.handle(r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#);
        let lines = out.take();
        assert_eq!(
            answer(&lines, 6)["result"]["content"][0]["text"],
            NOT_RUNNING
        );
        assert_eq!(answer(&lines, 6)["result"]["isError"], true);
        assert_eq!(tools_changed(&lines), 1, "{lines:?}");
        assert_eq!(answer(&lines, 7)["result"]["tools"], json!(catalog.tools));
        bridge.close();
    }

    /// The same version of Oxide: its list is the bridge's, so the client isn't told anything
    /// changed, and the watcher finds the app with no call made.
    #[test]
    fn the_watcher_finds_the_app_and_an_equal_list_changes_nothing() {
        let port = free_port();
        let out = Lines::default();
        let catalog = Catalog {
            instructions: String::new(),
            tools: vec![tool("a")],
            prompts: vec![],
        };
        let bridge = Bridge::with(Options { port, token: None }, catalog, "1.0.0", out.clone());
        bridge.handle(INIT);
        let init = out.take();
        assert_eq!(init[0]["result"]["instructions"], RUNNING_NOTE);
        let _app = FakeApp::start(port, vec![tool("a")], "1.0.0");
        let until = Instant::now() + Duration::from_secs(15);
        while bridge.upstream().app.is_none() {
            assert!(Instant::now() < until, "the watcher never found the app");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(tools_changed(&out.take()), 0);
        bridge.close();
    }
}
