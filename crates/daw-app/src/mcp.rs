//! The in-app MCP server: publishes `daw_core::api`'s action registry as MCP tools over
//! Streamable HTTP on localhost, so an AI agent can operate the running DAW.
//!
//! The server runs on its own tokio threads. Listing tools needs no round trip (the specs are
//! copied in at start). A tool call is sent to the UI thread, which owns the `Session`, wakes
//! it, and waits for the action's reply. The app drains calls in `App::logic`, which also runs
//! while the window is hidden.
//!
//! Starting and stopping (dev plan 17.2): [`Mcp`] is the app's side. It hands every start,
//! restart and stop to a worker of its own (`mcp-control`), which closes the old server (its
//! port free again) before binding the new one, so the UI thread never waits on a port or on
//! a server closing, and a port changed twice in a row only binds the last. The status
//! ([`Status`]) reads "MCP starting…" until the worker reports the address or why not ("port
//! 7878 is in use", with Retry). It does not depend on a project: the app starts it before
//! the start screen and keeps it through opens and new projects. While a project opens, calls
//! wait for it ([`Mcp::take_calls`]), except the few that report, stop or show the open.
//!
//! Stopping never leaves a caller hanging: calls not yet run are answered with an error at once
//! ([`Mcp::start`], [`Mcp::stop`]), a call still running gets one when its server stops (the
//! handler watches the server's stop), and only then are the clients' sessions closed.
//!
//! Access: the server binds to loopback only, rejects browser origins, and can also require a
//! bearer token (TD18): every request must then carry `Authorization: Bearer <token>`, or it
//! gets 401 before it reaches MCP. The token is an app setting, never an action, so an agent
//! can neither read it nor turn it off.

use base64::Engine as _;
use daw_core::api::{ActionOutput, ActionSpec, ApiError, Reply, Safety};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, GetPromptRequestParams,
    GetPromptResponse, GetPromptResult, Implementation, InitializeResult, ListPromptsResult,
    ListToolsResult, PaginatedRequestParams, Prompt, PromptArgument, PromptMessage, Role,
    ServerCapabilities, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use std::collections::VecDeque;
use std::future::IntoFuture as _;
use std::ops::RangeInclusive;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_PORT: u16 = 7878;

/// The ports the settings take: those below 1024 are the system's.
pub const PORTS: RangeInclusive<u16> = 1024..=65535;

/// How long a tool call may take on the UI thread. Opening a project reloads every plug-in,
/// which can take minutes for a large session. A call held while a project opens counts that
/// wait in this too. File dialogs don't hold calls (18.6): they run on a thread of their own.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);

/// How long a stopping server lets replies already on their way reach their clients before it
/// closes the clients' sessions (which ends their open streams, so the server can finish).
const REPLY_GRACE: Duration = Duration::from_millis(250);

/// How long a stopping server waits for its connections after closing the sessions.
const CLOSE_LIMIT: Duration = Duration::from_secs(2);

/// What a call still waiting is told when its server stops.
pub const STOPPED: &str = "the DAW's MCP server stopped (restarted or turned off in its \
                           settings) before this call was answered; it may not have run";

/// Calls answered at once while a project opens, and the changes allowed with no project
/// open: in daw-core, so the manual's reference can say which actions run when (27.6).
pub use daw_core::api::{ANSWERED_WHILE_OPENING, WITHOUT_A_PROJECT};

/// Why the app refuses a call (`safety`: the action's, `None` for an unknown one, which the
/// registry explains itself), or `None` to run it.
pub fn refusal(name: &str, safety: Option<Safety>, project_open: bool) -> Option<String> {
    let changes = safety.is_some_and(|s| s != Safety::Read);
    (changes && !project_open && !WITHOUT_A_PROJECT.contains(&name)).then(|| {
        format!(
            "{name} needs a project, and none is open in Oxide (it shows its start screen): \
             open one with open_project, or make one with new_project (with a name and folder)"
        )
    })
}

/// A tool call for the UI thread to run.
pub struct CallRequest {
    pub name: String,
    pub args: serde_json::Value,
    pub reply: Reply,
}

#[derive(Clone)]
struct Handler {
    tools: Arc<Vec<Tool>>,
    calls: Sender<CallRequest>,
    wake: egui::Context,
    /// Cancelled when the server stops: a call still waiting gets [`STOPPED`].
    stop: CancellationToken,
}

fn tool(spec: &ActionSpec) -> Tool {
    let schema = match &spec.input_schema {
        serde_json::Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    let annotations = ToolAnnotations::new()
        .read_only(spec.safety == Safety::Read)
        .destructive(spec.safety == Safety::Destructive);
    Tool::new(spec.name, spec.description, Arc::new(schema)).with_annotations(annotations)
}

/// The prompts as `prompts/list` gives them (the bridge's catalog has the same).
fn prompt_list() -> Vec<Prompt> {
    daw_core::api::prompts::prompts()
        .iter()
        .map(|p| {
            let arguments = p
                .arguments
                .iter()
                .map(|a| {
                    PromptArgument::new(a.name)
                        .with_description(a.description)
                        .with_required(a.required)
                })
                .collect();
            Prompt::new(p.name, Some(p.description), Some(arguments)).with_title(p.title)
        })
        .collect()
}

fn result(output: Result<ActionOutput, ApiError>) -> CallToolResult {
    match output {
        Ok(ActionOutput::Json(value)) => {
            let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
            CallToolResult::success(vec![ContentBlock::text(text)])
        }
        Ok(ActionOutput::Png(png)) => {
            let data = base64::engine::general_purpose::STANDARD.encode(png);
            CallToolResult::success(vec![ContentBlock::image(data, "image/png")])
        }
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e.0)]),
    }
}

impl ServerHandler for Handler {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        // The name Claude's configs give it (and the bridge answers with).
        .with_server_info(Implementation::new("oxide", crate::version::VERSION))
        .with_instructions(daw_core::api::INSTRUCTIONS)
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        Ok(ListPromptsResult::with_all_items(prompt_list()))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let prompt = daw_core::api::prompts::find(&request.name).ok_or_else(|| {
            ErrorData::invalid_params(
                format!(
                    "no prompt named {} (prompts/list has them: write_midi_part)",
                    request.name
                ),
                None,
            )
        })?;
        let text = prompt.text(&request.arguments.unwrap_or_default());
        Ok(
            GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)])
                .with_description(prompt.description)
                .into(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items((*self.tools).clone()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let stopped = || Ok(result(Err(ApiError(STOPPED.into()))).into());
        if self.stop.is_cancelled() {
            return stopped();
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let call = CallRequest {
            name: request.name.to_string(),
            args: request
                .arguments
                .map(serde_json::Value::Object)
                .unwrap_or_default(),
            reply: Box::new(move |output| {
                let _ = tx.send(output);
            }),
        };
        if self.calls.send(call).is_err() {
            // The app let go of this server's calls: it is restarting or closing.
            return stopped();
        }
        self.wake.request_repaint();
        let output = tokio::select! {
            replied = tokio::time::timeout(CALL_TIMEOUT, rx) => match replied {
                Ok(Ok(output)) => output,
                Ok(Err(_)) if self.stop.is_cancelled() => Err(ApiError(STOPPED.into())),
                Ok(Err(_)) => Err(ApiError("the DAW dropped the call".into())),
                Err(_) => Err(ApiError(format!(
                    "no reply within {} s (is a project still opening in the DAW?)",
                    CALL_TIMEOUT.as_secs()
                ))),
            },
            () = self.stop.cancelled() => Err(ApiError(STOPPED.into())),
        };
        Ok(result(output).into())
    }
}

/// Why the server could not start on a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Another program (or another Oxide) listens there.
    InUse,
    /// Windows keeps the port for itself (Hyper-V's excluded ranges) or forbids it.
    Reserved,
    /// Outside [`PORTS`] (a hand-edited settings file, `--mcp-port`).
    Invalid,
    Other(String),
}

/// The server's state, as the status bar and the settings show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Not running: turned off in the settings, or `why` (`--no-mcp`).
    Off {
        why: Option<String>,
    },
    /// Asked to start; the worker hasn't answered yet.
    Starting {
        port: u16,
    },
    Running {
        address: String,
        port: u16,
        /// Clients must send a token.
        token: bool,
    },
    Failed {
        port: u16,
        why: Failure,
    },
}

impl Status {
    /// The status bar's words.
    pub fn text(&self) -> String {
        match self {
            Self::Off { why: None } => "MCP off".into(),
            Self::Off { why: Some(why) } => format!("MCP off ({why})"),
            Self::Starting { .. } => "MCP starting…".into(),
            Self::Running { address, token, .. } => {
                format!("MCP {address}{}", if *token { " (token)" } else { "" })
            }
            Self::Failed { port, why } => match why {
                Failure::InUse => format!("MCP off: port {port} is in use"),
                Failure::Reserved => format!("MCP off: port {port} is reserved by Windows"),
                Failure::Invalid => format!(
                    "MCP off: port {port} is outside {}–{}",
                    PORTS.start(),
                    PORTS.end()
                ),
                Failure::Other(e) => format!("MCP off: port {port}: {e}"),
            },
        }
    }

    /// It failed to start (shown in red).
    pub fn failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    /// Starting again on the same port could help (Retry): not for a port it can't use.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Failed { why, .. } if *why != Failure::Invalid)
    }
}

/// The address the server serves on `port`.
pub fn address(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// Bind the loopback port, saying why not.
fn bind(port: u16) -> Result<std::net::TcpListener, Failure> {
    use std::io::ErrorKind;
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| match e.kind() {
            ErrorKind::AddrInUse => Failure::InUse,
            // WSAEACCES (10013): a port in one of Windows' excluded ranges.
            ErrorKind::PermissionDenied => Failure::Reserved,
            _ => Failure::Other(e.to_string()),
        })?;
    listener
        .set_nonblocking(true)
        .map_err(|e| Failure::Other(e.to_string()))?;
    Ok(listener)
}

/// A running server. Dropping it shuts the server down: the port is free again when the drop
/// returns, and every call still waiting has been answered with an error.
pub struct McpServer {
    pub address: String,
    stop: CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl McpServer {
    /// Start serving `specs` on `127.0.0.1:port` (0: any free port), requiring `token` if given.
    /// Calls arrive on the returned receiver. Binds on the calling thread: the app starts
    /// servers through [`Mcp`], on its worker; tests start them directly.
    #[cfg(test)]
    pub fn start(
        port: u16,
        token: Option<String>,
        specs: &[ActionSpec],
        wake: egui::Context,
    ) -> Result<(Self, Receiver<CallRequest>), Failure> {
        let (calls, receiver) = std::sync::mpsc::channel();
        let tools = Arc::new(specs.iter().map(tool).collect());
        let server = Self::serve(port, token, tools, calls, wake)?;
        Ok((server, receiver))
    }

    fn serve(
        port: u16,
        token: Option<String>,
        tools: Arc<Vec<Tool>>,
        calls: Sender<CallRequest>,
        wake: egui::Context,
    ) -> Result<Self, Failure> {
        let listener = bind(port)?;
        // The port bound (port 0 picks a free one).
        let port = listener
            .local_addr()
            .map_err(|e| Failure::Other(e.to_string()))?
            .port();
        let address = address(port);
        let stop = CancellationToken::new();
        let handler = Handler {
            tools,
            calls,
            wake,
            stop: stop.clone(),
        };
        // The clients' sessions (and their open streams) close on a token of their own, a
        // moment after the stop: replies to calls cut short by it still reach their clients.
        let sessions = CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.json_response = true;
        config.cancellation_token = sessions.clone();
        // Loopback hosts only (the default), and no web page may call in: a browser request
        // carries an Origin, and only local origins are allowed.
        config.allowed_origins = vec!["http://localhost:*".into(), "http://127.0.0.1:*".into()];
        let required = token;
        let stopping = stop.clone();
        let thread = std::thread::Builder::new()
            .name("mcp-server".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("mcp-worker")
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(e) => {
                        log::error!("MCP runtime: {e}");
                        return;
                    }
                };
                runtime.block_on(async move {
                    let service = StreamableHttpService::new(
                        move || Ok(handler.clone()),
                        Arc::new(LocalSessionManager::default()),
                        config,
                    );
                    let mut router = axum::Router::new().nest_service("/mcp", service);
                    if let Some(token) = required {
                        let token = Arc::new(token);
                        router = router.layer(axum::middleware::from_fn(
                            move |request: axum::extract::Request, next: axum::middleware::Next| {
                                let token = token.clone();
                                async move { require_token(&token, request, next).await }
                            },
                        ));
                    }
                    let listener = match tokio::net::TcpListener::from_std(listener) {
                        Ok(l) => l,
                        Err(e) => {
                            log::error!("MCP listener: {e}");
                            return;
                        }
                    };
                    // On the stop, axum drops the listener at once (the port is free) and
                    // waits for the connections; the sessions close a moment later, ending
                    // the streams that would otherwise keep it waiting.
                    let served = axum::serve(listener, router)
                        .with_graceful_shutdown(stopping.clone().cancelled_owned())
                        .into_future();
                    let mut served = std::pin::pin!(served);
                    tokio::select! {
                        done = &mut served => {
                            if let Err(e) = done {
                                log::error!("MCP server: {e}");
                            }
                        }
                        () = async {
                            stopping.cancelled().await;
                            tokio::time::sleep(REPLY_GRACE).await;
                        } => {
                            sessions.cancel();
                            if tokio::time::timeout(CLOSE_LIMIT, served).await.is_err() {
                                log::warn!("MCP server: connections still open after stopping");
                            }
                        }
                    }
                });
                runtime.shutdown_timeout(Duration::from_millis(200));
            })
            .map_err(|e| Failure::Other(e.to_string()))?;
        Ok(Self {
            address,
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the app asks its worker.
enum Command {
    Start {
        generation: u64,
        port: u16,
        token: Option<String>,
        calls: Sender<CallRequest>,
    },
    Stop {
        generation: u64,
    },
}

/// The worker's answer to a command: the address served, or why not (`None` for a stop).
type Report = (u64, Option<Result<String, Failure>>);

/// The app's side of the server (see the module's docs). Lives on the UI thread; every bind
/// and every close happens on its worker.
pub struct Mcp {
    commands: Sender<Command>,
    reports: Receiver<Report>,
    /// Bumped by every start and stop: a report for an older one is stale.
    generation: u64,
    status: Status,
    /// The port and token last asked for (Retry).
    asked: Option<(u16, Option<String>)>,
    /// This server's calls: made with the start, so none is lost while it binds.
    calls: Option<Receiver<CallRequest>>,
    /// Calls waiting for a project to open, oldest first.
    held: VecDeque<CallRequest>,
}

impl Mcp {
    /// The worker, ready, with nothing started. `specs` become tools on it, once.
    pub fn new(specs: Vec<ActionSpec>, wake: egui::Context) -> Self {
        let (commands, inbox) = std::sync::mpsc::channel();
        let (outbox, reports) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("mcp-control".into())
            .spawn(move || control(&inbox, &outbox, &specs, &wake));
        if let Err(e) = spawned {
            log::error!("MCP: no worker: {e}");
        }
        Self {
            commands,
            reports,
            generation: 0,
            status: Status::Off { why: None },
            asked: None,
            calls: None,
            held: VecDeque::new(),
        }
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Start (or restart) the server on `port`, requiring `token` if given. Returns at once:
    /// the status says "starting" until the worker has closed the running server and bound
    /// the port, or failed to. Calls the old server took and the app hasn't run get an error.
    pub fn start(&mut self, port: u16, token: Option<String>) {
        self.let_go("the DAW's MCP server is restarting; call again once it is up");
        self.generation += 1;
        self.asked = Some((port, token.clone()));
        if !PORTS.contains(&port) {
            self.status = Status::Failed {
                port,
                why: Failure::Invalid,
            };
            self.send(Command::Stop {
                generation: self.generation,
            });
            return;
        }
        let (calls, receiver) = std::sync::mpsc::channel();
        self.calls = Some(receiver);
        self.status = Status::Starting { port };
        self.send(Command::Start {
            generation: self.generation,
            port,
            token,
            calls,
        });
    }

    /// Start again with what was last asked (after a failure, once the port is free).
    pub fn retry(&mut self) {
        if let Some((port, token)) = self.asked.clone() {
            self.start(port, token);
        }
    }

    /// Stop the server (`why`: shown after "MCP off"). Returns at once; calls not yet run get
    /// an error.
    pub fn stop(&mut self, why: Option<String>) {
        self.let_go("the DAW's MCP server was turned off");
        self.generation += 1;
        self.status = Status::Off { why };
        self.send(Command::Stop {
            generation: self.generation,
        });
    }

    fn send(&mut self, command: Command) {
        if self.commands.send(command).is_err() {
            self.status = Status::Failed {
                port: self.asked.as_ref().map_or(0, |a| a.0),
                why: Failure::Other("its worker is gone".into()),
            };
        }
    }

    /// Answer every call not yet run with `why`, and stop taking the old server's calls.
    fn let_go(&mut self, why: &str) {
        let queued: Vec<CallRequest> = self
            .calls
            .take()
            .map(|c| c.try_iter().collect())
            .unwrap_or_default();
        for call in self.held.drain(..).chain(queued) {
            (call.reply)(Err(ApiError(why.into())));
        }
    }

    /// Take in the worker's answers; true when the status changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok((generation, report)) = self.reports.try_recv() {
            let Status::Starting { port } = self.status else {
                continue;
            };
            let Some(report) = report.filter(|_| generation == self.generation) else {
                continue;
            };
            self.status = match report {
                Ok(address) => Status::Running {
                    address,
                    port,
                    token: self.asked.as_ref().is_some_and(|a| a.1.is_some()),
                },
                Err(why) => Status::Failed { port, why },
            };
            changed = true;
        }
        changed
    }

    /// The calls to run now. While a project opens (`opening`) only
    /// [`ANSWERED_WHILE_OPENING`] run; the rest wait, in order, until it is open.
    pub fn take_calls(&mut self, opening: bool) -> Vec<CallRequest> {
        let incoming: Vec<CallRequest> = self
            .calls
            .as_ref()
            .map(|c| c.try_iter().collect())
            .unwrap_or_default();
        split_calls(&mut self.held, incoming, opening)
    }

    /// Wait (up to `limit`) for the worker to answer the last start: tests.
    #[cfg(test)]
    pub fn settle(&mut self, limit: Duration) -> &Status {
        let until = std::time::Instant::now() + limit;
        while matches!(self.status, Status::Starting { .. }) && std::time::Instant::now() < until {
            self.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        &self.status
    }
}

/// [`Mcp::take_calls`]' rule: what runs now, and what waits in `held`.
fn split_calls(
    held: &mut VecDeque<CallRequest>,
    incoming: Vec<CallRequest>,
    opening: bool,
) -> Vec<CallRequest> {
    if !opening {
        return held.drain(..).chain(incoming).collect();
    }
    let mut now = Vec::new();
    for call in incoming {
        if ANSWERED_WHILE_OPENING.contains(&call.name.as_str()) {
            now.push(call);
        } else {
            held.push_back(call);
        }
    }
    now
}

/// The worker: one start or stop at a time, the old server closed before the next binds.
fn control(
    inbox: &Receiver<Command>,
    outbox: &Sender<Report>,
    specs: &[ActionSpec],
    wake: &egui::Context,
) {
    let tools: Arc<Vec<Tool>> = Arc::new(specs.iter().map(tool).collect());
    let mut server: Option<McpServer> = None;
    while let Ok(mut command) = inbox.recv() {
        // Only the newest matters (the port changed again before the first was bound).
        while let Ok(newer) = inbox.try_recv() {
            command = newer;
        }
        // The old server closes first: its port is free before anything binds again.
        drop(server.take());
        let report = match command {
            Command::Start {
                generation,
                port,
                token,
                calls,
            } => match McpServer::serve(port, token, tools.clone(), calls, wake.clone()) {
                Ok(started) => {
                    let address = started.address.clone();
                    server = Some(started);
                    (generation, Some(Ok(address)))
                }
                Err(why) => (generation, Some(Err(why))),
            },
            Command::Stop { generation } => (generation, None),
        };
        if outbox.send(report).is_err() {
            break;
        }
        wake.request_repaint();
    }
    // The app is gone: the server closes with this thread.
}

// Pass the request on only if it carries the bearer token; otherwise 401.
async fn require_token(
    token: &str,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if authorized(header, token) {
        next.run(request).await
    } else {
        axum::response::Response::builder()
            .status(axum::http::StatusCode::UNAUTHORIZED)
            .header(axum::http::header::WWW_AUTHENTICATE, "Bearer")
            .body(axum::body::Body::from(
                "this DAW requires a token: Authorization: Bearer <token>",
            ))
            .unwrap_or_default()
    }
}

/// Whether an `Authorization` header carries `token` as a bearer token. Compares in constant
/// time, so the reply's timing doesn't give the token away a character at a time.
pub fn authorized(header: Option<&str>, token: &str) -> bool {
    let Some(given) = header.and_then(|h| {
        h.strip_prefix("Bearer ")
            .or_else(|| h.strip_prefix("bearer "))
    }) else {
        return false;
    };
    let (given, token) = (given.trim().as_bytes(), token.as_bytes());
    given.len() == token.len() && given.iter().zip(token).fold(0, |d, (a, b)| d | (a ^ b)) == 0
}

/// A new random token: 32 bytes from the OS, as 64 hex digits.
pub fn new_token() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness for a token: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The command that connects Claude Code to the server at `address`, under the name and in
/// the scope the settings' Configure uses (`mcp_link::clients::NAME`).
pub fn connect_command(address: &str, token: Option<&str>) -> String {
    let name = mcp_link::clients::NAME;
    match token {
        Some(token) => format!(
            "claude mcp add --transport http --scope user {name} {address} --header \"Authorization: Bearer {token}\""
        ),
        None => format!("claude mcp add --transport http --scope user {name} {address}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::time::Instant;

    /// The worker answers within this (it binds in microseconds; a debug build is slower).
    const SETTLE: Duration = Duration::from_secs(10);

    fn registry_specs() -> Vec<ActionSpec> {
        daw_core::api::Registry::new().specs()
    }

    /// A loopback port nothing listens on, for one test alone. Not one the system hands out
    /// (`bind(0)`, the ephemeral range from 49152), so another test's `bind(0)` or outgoing
    /// connection can't take it between this test checking it and binding it: ports from
    /// 37000 by process, then a counter, each checked free (Hyper-V's reserved ranges skipped).
    fn free_port() -> u16 {
        static NEXT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
        let base = 37_000 + (std::process::id() % 500) as u16 * 8;
        loop {
            let port = base + NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst) % 4_000;
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return port;
            }
        }
    }

    /// Send one JSON-RPC message (in an MCP session, if given); the open stream, unread.
    fn send(address: &str, body: &str, session: Option<&str>) -> std::net::TcpStream {
        let host = address
            .trim_start_matches("http://")
            .trim_end_matches("/mcp");
        let mut stream = std::net::TcpStream::connect(host).unwrap();
        let mut lines = vec![
            "POST /mcp HTTP/1.1".to_string(),
            format!("Host: {host}"),
            "Content-Type: application/json".into(),
            "Accept: application/json, text/event-stream".into(),
            format!("Content-Length: {}", body.len()),
            "Connection: close".into(),
        ];
        lines.extend(session.map(|s| format!("Mcp-Session-Id: {s}")));
        write!(stream, "{}\r\n\r\n{body}", lines.join("\r\n")).unwrap();
        stream
    }

    /// The whole reply on a stream (with a limit, so a hang fails the test instead).
    fn read_all(mut stream: std::net::TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    }

    /// An initialised MCP session on the server at `address`: its id.
    fn session(address: &str) -> String {
        let reply = read_all(send(
            address,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
            None,
        ));
        assert!(reply.contains("\"serverInfo\""), "{reply}");
        let id = reply
            .lines()
            .find_map(|l| {
                let (name, value) = l.split_once(':')?;
                name.eq_ignore_ascii_case("mcp-session-id")
                    .then(|| value.trim().to_string())
            })
            .expect("a session id");
        read_all(send(
            address,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            Some(&id),
        ));
        id
    }

    /// Ask for a tool call (unanswered until the app runs it): the open stream. Each call in a
    /// session needs its own `id`.
    fn call(address: &str, session: &str, id: u32, name: &str) -> std::net::TcpStream {
        send(
            address,
            &format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{{}}}}}}"#
            ),
            Some(session),
        )
    }

    /// The app's frame: run what arrived (answering every call with `answer`).
    fn answer_calls(mcp: &mut Mcp, answer: &str) -> usize {
        let calls = mcp.take_calls(false);
        let n = calls.len();
        for call in calls {
            (call.reply)(Ok(ActionOutput::Json(serde_json::json!(answer))));
        }
        n
    }

    #[test]
    fn starting_returns_at_once_and_a_taken_port_says_so_until_retried() {
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let mut mcp = Mcp::new(registry_specs(), egui::Context::default());
        let asked = Instant::now();
        mcp.start(port, None);
        let took = asked.elapsed();
        // The frame goes on: nothing bound or built on the calling thread.
        assert!(took < Duration::from_millis(20), "start took {took:?}");
        assert_eq!(mcp.status(), &Status::Starting { port });
        assert_eq!(mcp.status().text(), "MCP starting…");
        let status = mcp.settle(SETTLE).clone();
        assert_eq!(
            status,
            Status::Failed {
                port,
                why: Failure::InUse
            }
        );
        assert_eq!(status.text(), format!("MCP off: port {port} is in use"));
        assert!(status.failed() && status.retryable());
        // The other program lets go: Retry starts it on the same port.
        drop(held);
        mcp.retry();
        assert_eq!(mcp.status(), &Status::Starting { port });
        let status = mcp.settle(SETTLE).clone();
        let address = address(port);
        assert_eq!(
            status,
            Status::Running {
                address: address.clone(),
                port,
                token: false
            }
        );
        // It answers: a session, and a call through to the app.
        let id = session(&address);
        let pending = call(&address, &id, 2, "get_project");
        let until = Instant::now() + SETTLE;
        while answer_calls(&mut mcp, "answered") == 0 {
            assert!(Instant::now() < until, "the call never reached the app");
            std::thread::sleep(Duration::from_millis(2));
        }
        let reply = read_all(pending);
        assert!(reply.contains("answered"), "{reply}");
    }

    #[test]
    fn changing_the_port_restarts_there_frees_the_old_one_and_errs_calls_in_flight() {
        let (a, b) = (free_port(), free_port());
        let mut mcp = Mcp::new(registry_specs(), egui::Context::default());
        mcp.start(a, None);
        assert!(matches!(mcp.settle(SETTLE), Status::Running { port, .. } if *port == a));
        let old = address(a);
        let id = session(&old);
        // One call the app has taken and not answered yet (a deferred action), one it hasn't
        // taken: both must get an error when the server moves, never hang.
        let running = call(&old, &id, 2, "get_project");
        let until = Instant::now() + SETTLE;
        let mut taken = Vec::new();
        while taken.is_empty() {
            assert!(Instant::now() < until, "the call never reached the app");
            taken = mcp.take_calls(false);
            std::thread::sleep(Duration::from_millis(2));
        }
        let queued = call(&old, &id, 3, "get_tempo_map");
        std::thread::sleep(Duration::from_millis(100));
        let asked = Instant::now();
        mcp.start(b, None);
        assert!(asked.elapsed() < Duration::from_millis(20));
        let status = mcp.settle(SETTLE).clone();
        assert!(
            matches!(&status, Status::Running { port, .. } if *port == b),
            "{status:?}"
        );
        // Both callers were told.
        let running = read_all(running);
        assert!(running.contains("\"isError\":true"), "{running}");
        assert!(running.contains("stopped"), "{running}");
        // (Told by the app if the call reached it before the restart, else by the server.)
        let queued = read_all(queued);
        assert!(queued.contains("\"isError\":true"), "{queued}");
        assert!(
            queued.contains("restarting") || queued.contains("stopped"),
            "{queued}"
        );
        // The old port is free again; the new one answers.
        drop(std::net::TcpListener::bind(("127.0.0.1", a)).expect("the old port is still held"));
        session(&address(b));
        drop(taken);
        // Turned off: the port is let go too.
        mcp.stop(None);
        assert_eq!(mcp.status().text(), "MCP off");
        let until = Instant::now() + SETTLE;
        while std::net::TcpListener::bind(("127.0.0.1", b)).is_err() {
            assert!(Instant::now() < until, "port {b} still held after stopping");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_port_outside_the_range_is_refused_with_its_reason() {
        let mut mcp = Mcp::new(Vec::new(), egui::Context::default());
        mcp.start(80, None);
        assert_eq!(
            mcp.status().text(),
            "MCP off: port 80 is outside 1024–65535"
        );
        // Retrying 80 can't help: the port is what to change.
        assert!(mcp.status().failed() && !mcp.status().retryable());
        mcp.stop(Some("--no-mcp".into()));
        assert_eq!(mcp.status().text(), "MCP off (--no-mcp)");
    }

    #[test]
    fn calls_wait_while_a_project_opens_except_those_that_report_it() {
        let names = std::cell::RefCell::new(Vec::new());
        let request = |name: &str| CallRequest {
            name: name.into(),
            args: serde_json::Value::Null,
            reply: Box::new(|_| {}),
        };
        let mut held = VecDeque::new();
        let mut run = |incoming: Vec<CallRequest>, opening: bool| {
            let now = split_calls(&mut held, incoming, opening);
            names
                .borrow_mut()
                .push(now.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
        };
        run(
            vec![
                request("add_track"),
                request("get_busy"),
                request("get_project"),
            ],
            true,
        );
        run(vec![request("cancel_open"), request("set_tempo")], true);
        run(vec![request("screenshot")], false);
        assert_eq!(
            names.into_inner(),
            vec![
                vec!["get_busy"],
                vec!["cancel_open"],
                // In the order they came, the held ones first.
                vec!["add_track", "get_project", "set_tempo", "screenshot"],
            ]
        );
        // Every name answered while opening is an action.
        let specs = registry_specs();
        for name in ANSWERED_WHILE_OPENING {
            assert!(specs.iter().any(|s| s.name == *name), "no action {name}");
        }
    }

    #[test]
    fn with_no_project_open_only_reads_and_project_free_changes_run() {
        let registry = daw_core::api::Registry::new();
        let refused = |name: &str, open: bool| refusal(name, registry.safety(name), open);
        // Reads, and making or opening a project, run on the start screen.
        for name in [
            "get_busy",
            "get_project",
            "list_plugins",
            "new_project",
            "open_project",
            "save_project",
            "search_manual",
            "open_manual",
        ] {
            assert_eq!(refused(name, false), None, "{name}");
        }
        // Changes to a project don't, and say why.
        for name in [
            "add_track",
            "set_tempo",
            "play",
            "remove_track",
            "import_audio",
        ] {
            let why = refused(name, false).unwrap_or_else(|| panic!("{name} ran"));
            assert!(
                why.contains("none is open") && why.contains("new_project"),
                "{why}"
            );
            assert_eq!(refused(name, true), None, "{name} with a project");
        }
        // An unknown name is left to the registry ("unknown action").
        assert_eq!(refused("no_such_action", false), None);
        // Every name allowed is an action that changes something.
        for name in WITHOUT_A_PROJECT {
            let safety = registry.safety(name);
            assert!(
                safety.is_some_and(|s| s != Safety::Read),
                "{name}: {safety:?}"
            );
        }
    }

    #[test]
    fn only_the_exact_bearer_token_is_let_in() {
        let token = "0123abcd";
        assert!(authorized(Some("Bearer 0123abcd"), token));
        assert!(authorized(Some("bearer 0123abcd "), token));
        assert!(!authorized(None, token));
        assert!(!authorized(Some("0123abcd"), token));
        assert!(!authorized(Some("Bearer 0123abc"), token));
        assert!(!authorized(Some("Bearer 0123abce"), token));
        assert!(!authorized(Some("Basic 0123abcd"), token));
    }

    #[test]
    fn tokens_are_random_hex() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(connect_command("http://x/mcp", Some(&a)).ends_with(&format!("Bearer {a}\"")));
        assert!(!connect_command("http://x/mcp", None).contains("--header"));
    }

    /// POST an empty body to the server; returns the reply's status code.
    fn post(address: &str, authorization: Option<&str>) -> u16 {
        let host = address
            .trim_start_matches("http://")
            .trim_end_matches("/mcp");
        let mut stream = std::net::TcpStream::connect(host).unwrap();
        let mut lines = vec![
            "POST /mcp HTTP/1.1".to_string(),
            format!("Host: {host}"),
            "Content-Type: application/json".into(),
            "Accept: application/json, text/event-stream".into(),
            "Content-Length: 2".into(),
            "Connection: close".into(),
        ];
        lines.extend(authorization.map(|a| format!("Authorization: {a}")));
        write!(stream, "{}\r\n\r\n{{}}", lines.join("\r\n")).unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply.split(' ').nth(1).unwrap().parse().unwrap()
    }

    #[test]
    fn a_server_with_a_token_turns_away_requests_without_it() {
        let token = new_token().unwrap();
        let (server, _calls) =
            McpServer::start(0, Some(token.clone()), &[], egui::Context::default()).unwrap();
        assert_eq!(post(&server.address, None), 401);
        assert_eq!(post(&server.address, Some("Bearer wrong")), 401);
        // With the token, the request reaches MCP (which dislikes the empty message, but not
        // with a 401).
        assert_ne!(post(&server.address, Some(&format!("Bearer {token}"))), 401);
        let (open, _calls) = McpServer::start(0, None, &[], egui::Context::default()).unwrap();
        assert_ne!(post(&open.address, None), 401);
    }
    /// What the bridge writes, one JSON message a line.
    #[derive(Clone, Default)]
    struct Lines(Arc<std::sync::Mutex<Vec<u8>>>);

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
        fn take(&self) -> Vec<serde_json::Value> {
            let bytes = std::mem::take(&mut *self.0.lock().unwrap());
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        }
    }

    /// What a tools/call through a bridge came back with: its text, and whether it is an error.
    fn called(lines: &[serde_json::Value], id: i64) -> (String, bool) {
        let m = lines
            .iter()
            .find(|m| m["id"] == id)
            .unwrap_or_else(|| panic!("no answer to {id}: {lines:?}"));
        assert!(m.get("error").is_none(), "a protocol error: {m}");
        (
            m["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            m["result"]["isError"] == true,
        )
    }

    /// Answer the next `n` calls that reach "the UI thread" with `{"tracks": []}`.
    fn answer_next(calls: Receiver<CallRequest>, n: usize) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            for _ in 0..n {
                let call = calls
                    .recv_timeout(SETTLE)
                    .expect("the call reaches the app");
                (call.reply)(Ok(ActionOutput::Json(serde_json::json!({"tracks": []}))));
            }
        })
    }

    const BRIDGE_INIT: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-ai","version":"1"}}}"#;

    fn tool_call(id: i64, name: &str) -> String {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": {}}}).to_string()
    }

    /// The bridge's catalog (dev plan 23.3) is what this build's server publishes: every tool
    /// as `tools/list` has it, the prompts and the instructions.
    #[test]
    fn the_bridges_catalog_is_what_the_server_publishes() {
        let catalog = mcp_link::bridge::Catalog::embedded();
        let stale = "crates/mcp-link/src/catalog.json is stale: regenerate it with UPDATE_DOCS=1 \
                     (PowerShell: $env:UPDATE_DOCS='1'; cargo test -p daw-core docs_are_current; \
                     Remove-Item Env:\\UPDATE_DOCS)";
        let tools: Vec<serde_json::Value> = registry_specs()
            .iter()
            .map(|s| serde_json::to_value(tool(s)).unwrap())
            .collect();
        assert_eq!(tools.len(), catalog.tools.len(), "{stale}");
        for (ours, listed) in tools.iter().zip(&catalog.tools) {
            assert_eq!(ours, listed, "{stale}");
        }
        assert_eq!(
            serde_json::to_value(prompt_list()).unwrap(),
            serde_json::json!(catalog.prompts),
            "{stale}"
        );
        assert_eq!(catalog.instructions, daw_core::api::INSTRUCTIONS, "{stale}");
    }

    /// Claude started before Oxide (23.3): the bridge lists the registry's tools and a call says
    /// Oxide isn't running (a tool error); the server starts on the port and the next call goes
    /// through the same bridge, whose list was the server's all along (no `list_changed`).
    #[test]
    fn the_bridge_lists_the_servers_tools_and_reaches_it_once_started() {
        use mcp_link::bridge::{Bridge, Catalog, Options, NOT_RUNNING, RUNNING_NOTE};
        let port = free_port();
        let out = Lines::default();
        let bridge = Bridge::new(Options { port, token: None }, out.clone());
        bridge.handle(BRIDGE_INIT);
        bridge.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        bridge.handle(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        bridge.handle(r#"{"jsonrpc":"2.0","id":2,"method":"prompts/list"}"#);
        bridge.handle(&tool_call(3, "get_project"));
        let lines = out.take();
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "oxide");
        let instructions = lines[0]["result"]["instructions"].as_str().unwrap();
        assert!(
            instructions.starts_with(daw_core::api::INSTRUCTIONS)
                && instructions.ends_with(RUNNING_NOTE)
        );
        let offline = lines[1]["result"]["tools"].clone();
        assert_eq!(offline.as_array().unwrap().len(), registry_specs().len());
        assert!(lines[2]["result"]["prompts"][0]["name"] == "write_midi_part");
        assert_eq!(
            called(&lines, 3),
            (NOT_RUNNING.to_string(), true),
            "a tool error, not a protocol error"
        );

        // Oxide starts: the same bridge reaches it.
        let specs = registry_specs();
        let (server, calls) =
            McpServer::start(port, None, &specs, egui::Context::default()).unwrap();
        let answering = answer_next(calls, 2);
        bridge.handle(&tool_call(4, "get_project"));
        let lines = out.take();
        let (text, error) = called(&lines, 4);
        assert!(!error && text.contains("tracks"), "{text}");
        assert!(
            !lines
                .iter()
                .any(|m| m["method"] == "notifications/tools/list_changed"),
            "the server's list is the bridge's: {lines:?}"
        );
        // What the server lists, as a bridge with nothing of its own sees it once connected.
        let seen = Lines::default();
        let probe = Bridge::with(
            Options { port, token: None },
            Catalog {
                instructions: String::new(),
                tools: Vec::new(),
                prompts: Vec::new(),
            },
            "0.0.0",
            seen.clone(),
        );
        probe.handle(BRIDGE_INIT);
        probe.handle(&tool_call(1, "get_project"));
        probe.handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        answering.join().unwrap();
        let lines = seen.take();
        let served = &lines.iter().find(|m| m["id"] == 2).unwrap()["result"]["tools"];
        assert_eq!(served, &offline, "the bridge lists what the server serves");
        probe.close();
        bridge.close();
        drop(server);
    }

    /// The bridge against the real server with a token: calls relayed (answered as the UI
    /// thread would), a server restarted on the same port picked up again by the same bridge,
    /// and tool errors when the token is wrong or the app is gone.
    #[test]
    fn the_bridge_relays_to_the_server_and_follows_it_through_a_restart() {
        use mcp_link::bridge::{Bridge, Options, NOT_RUNNING};
        let token = new_token().unwrap();
        let specs = registry_specs();
        let (server, calls) =
            McpServer::start(0, Some(token.clone()), &specs, egui::Context::default()).unwrap();
        let port: u16 = server
            .address
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches("/mcp")
            .parse()
            .unwrap();
        let out = Lines::default();
        let bridge = Bridge::new(
            Options {
                port,
                token: Some(token.clone()),
            },
            out.clone(),
        );
        bridge.handle(BRIDGE_INIT);
        bridge.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        let answering = answer_next(calls, 1);
        bridge.handle(&tool_call(2, "get_project"));
        answering.join().unwrap();
        let lines = out.take();
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "oxide");
        let (text, error) = called(&lines, 2);
        assert!(!error && text.contains("tracks"), "{text}");

        // The app restarts on the same port: the old session is unknown there, so the bridge
        // opens a new one with the client's initialize and asks again.
        drop(server);
        let (server, calls) =
            McpServer::start(port, Some(token.clone()), &specs, egui::Context::default()).unwrap();
        let answering = answer_next(calls, 1);
        bridge.handle(&tool_call(3, "get_project"));
        answering.join().unwrap();
        let (text, error) = called(&out.take(), 3);
        assert!(!error && text.contains("tracks"), "{text}");

        // A bridge with an old token is told what to do.
        let stale = Lines::default();
        let wrong = Bridge::new(
            Options {
                port,
                token: Some("0".repeat(64)),
            },
            stale.clone(),
        );
        wrong.handle(BRIDGE_INIT);
        wrong.handle(&tool_call(1, "get_project"));
        let (text, error) = called(&stale.take(), 1);
        assert!(error && text.contains("Reconfigure Claude Code"), "{text}");
        wrong.close();

        // The app closed: calls say so.
        drop(server);
        bridge.handle(&tool_call(4, "get_project"));
        assert_eq!(called(&out.take(), 4), (NOT_RUNNING.to_string(), true));
        bridge.close();
    }
}
