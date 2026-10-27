# Claude can't reach Oxide (MCP)

Oxide runs an MCP server so an AI agent such as Claude Code or Claude Desktop can operate it.
When Claude can't connect, says it can't find Oxide, or its Oxide tools fail, check these in
order.

## 1. Is the server running?

Look at the status bar:

::: shot mcp-port-in-use
crop status.threads status.midi 6
highlight status.mcp "1" "Why MCP is off: here, its port is taken."
highlight status.mcp_retry "2" "Retry starts it again."
The status bar when MCP's port is in use.
:::

| the status bar says | it means |
|---|---|
| *MCP http://127.0.0.1:7878/mcp* | Running, on that address. *(token)* after it: clients must send the token. |
| *MCP starting…* | Starting; wait a moment. |
| *MCP off* | Switched off in the settings. |
| *MCP off: port 7878 is in use* | Another program (or another Oxide) has the port. Close it and press **Retry**, or choose another port. |
| *MCP off: port 7878 is reserved by Windows* | Windows keeps that port for itself. Choose another port. |

Click the MCP item to open Options > Settings… > **AI control (MCP)** (Options > AI control
(MCP)… opens it too). There:

- **Enabled** switches the server on and off.
- **Port** is where it listens (1024 to 65535; 7878 unless you change it). A new port applies
  when you press Enter or click elsewhere.
- **Status** says the same as the status bar, with **Retry** after a failure.

Only programs on this computer can connect.

## 2. Is Claude configured for this Oxide?

Under **Connect Claude**, a row each for **Claude Code** and **Claude Desktop** says how
Claude is set up, with **Configure**, **Reconfigure** and **Remove**:

| the row says | do |
|---|---|
| *Not configured* | **Configure**. |
| *Configured (port 7878)* | Nothing: it is set up for this port. |
| *Configured for another port (…)* | **Reconfigure**, for the port Oxide uses now. |
| *Configured (port …) with an old token* | **Reconfigure**: the token changed. |
| *Configured …, but its bridge is missing* or *out of date* | **Reconfigure**: it copies the bridge again. |
| *Configured over HTTP …: Reconfigure to use the bridge* | **Reconfigure**, so Oxide stays listed while it is closed. |
| *Not installed* | That Claude isn't installed (or, for Claude Code, its `claude` command isn't on the PATH). |

**Check again** reads Claude's settings again after you changed them elsewhere.

Configure sets Claude up to run `oxide-mcp.exe`, a small bridge beside Oxide. Through it, Claude
lists Oxide's tools even while Oxide is closed; a tool called then answers that Oxide isn't
running (after about two seconds), and works as soon as Oxide is started.

## 3. Restart Claude

- **Claude Desktop** reads its settings only when it starts. After Configure or Reconfigure,
  quit Claude Desktop completely (closing its window may leave it running) and start it
  again. The Configure message says so: *Restart Claude Desktop to use it*.
- **Claude Code** reads them when a session starts: start a new session.

## The token

With **Require a token** ticked, clients must send it; a request without it is refused. The
token is never shown whole; **Copy** beside the command copies the command with it. **New
token** locks out every client using the old one at once: press **Reconfigure** for each
client afterwards. Agents can't read or change the token.

## Connecting by hand

The **Command** line shows the `claude mcp add` command that connects Claude Code over HTTP,
for the port in use (**Copy** copies it, token included). Connected that way, Claude Code finds
Oxide only if Oxide is running when the session starts; Connect Claude's bridge doesn't need
that.

## See also

- [[page:mcp-reference]]: every action Claude can call.
- [[page:index#opening-the-manual]]: an agent can search this manual too.
