//! `oxide-mcp --port N [--token T]`: the bridge Claude Desktop and Claude Code run to reach
//! Oxide's MCP server (dev plan 17.3, 23.3). MCP on stdin and stdout, forwarded to
//! `http://127.0.0.1:N/mcp`; the tools and prompts are listed from the catalog built into it
//! (`mcp-link/src/catalog.json`), so Claude loads it while Oxide is closed. See
//! `mcp_link::bridge`. Oxide's settings (Options > AI control (MCP), Configure) write it into
//! Claude's configs. It writes nothing to disk; diagnostics go to stderr.

use mcp_link::bridge;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", bridge::USAGE);
        return ExitCode::SUCCESS;
    }
    let options = match bridge::parse_args(args) {
        Ok(options) => options,
        Err(e) => {
            eprintln!("oxide-mcp: {e}\n\n{}", bridge::USAGE);
            return ExitCode::from(2);
        }
    };
    let bridge = bridge::Bridge::with(
        options,
        bridge::Catalog::embedded(),
        // The app's own version (a check's build may override the workspace's).
        env!("OXIDE_VERSION"),
        std::io::stdout(),
    );
    bridge::run(&bridge, std::io::stdin().lock());
    ExitCode::SUCCESS
}
