//! The link between Oxide's MCP server and Claude's apps (dev plan 17.3).
//!
//! - [`clients`]: where Claude Code's and Claude Desktop's configs are, whether Oxide is in
//!   them ([`clients::detect`]), and Configure, Reconfigure and Remove ([`clients::apply`]).
//!   The app's settings call these on a worker, only when the user clicks; there is no MCP
//!   action for them (an agent must not rewrite its own client's config, and the token never
//!   leaves the app).
//! - [`bridge`]: `oxide-mcp.exe` (a binary of `daw-app`, built beside `daw.exe`), which Claude
//!   Desktop and Claude Code run: MCP on stdio, forwarded to the app's Streamable HTTP
//!   endpoint, with the tools listed from its own catalog while the app is closed (23.3).
//! - [`json_edit`]: changing one member of a client's JSON file and no other byte.
//!
//! Only std and serde_json, so the bridge stays small.

pub mod bridge;
pub mod clients;
pub mod json_edit;
