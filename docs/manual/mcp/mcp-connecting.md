# Connecting Claude

Oxide serves MCP on your computer while it runs, at `http://127.0.0.1:7878/mcp` unless you
change the port. Claude Code and Claude Desktop don't talk to that address directly: Oxide sets
each of them up to run a small program beside it, `oxide-mcp.exe`, which relays Claude's calls.
That is what lets Claude list Oxide's tools while Oxide is closed, and use them as soon as it
is started.

You do this once. [[page:settings#connect-claude]] has the buttons; this page says what they
do and what to do by hand.

## From Oxide's settings

1. Open **Options > AI control (MCP)…** (or **Options > Settings…** and pick *AI control
   (MCP)*). Check that **Enabled** is ticked and the status says *MCP http://127.0.0.1:7878/mcp*.
2. Under **Connect Claude**, click **Configure** on the row of the Claude you use, or both.
3. Start a new Claude Code session, or quit Claude Desktop completely and start it again.
4. Ask Claude about your project ("what tracks are in my song?"). It answers from
   [[action:get_project]].

Each row says where its Claude stands: *Not installed*, *Not configured*, *Configured (port
7878)*, or what needs changing (another port, an old token, a missing or out-of-date bridge, an
entry that still goes over HTTP). **Reconfigure** puts it right, **Remove** takes Oxide out, and
**Check again** reads Claude's settings again after you changed them elsewhere. The rows are
explained on [[page:settings#connect-claude]], and each state's fix on [[page:mcp-not-connecting]].

## Claude Code

**Configure** runs Claude Code's own `claude mcp add` command for you, at user scope, so Oxide
is there in every project you open with Claude Code. It needs the `claude` command on your
PATH; without it the row says *Not installed*.

Claude Code reads its servers when a session starts. After Configure, Reconfigure or Remove,
start a new session.

## Claude Desktop

**Configure** adds Oxide to Claude Desktop's settings file. It changes only Oxide's entry
(every other byte of the file stays as it was), keeps a backup, and replaces the file in one
step, so a failure can't leave it half written.

Claude Desktop reads its settings only when it starts, so after **Configure** or
**Reconfigure** the message says *Restart Claude Desktop to use it*. Restarting means quitting
it completely, not closing its window, which can leave it running in the background, and then
starting it again.

## Connecting by hand

The **Command** row in the settings shows the `claude mcp add` line for the port in use, and
**Copy** copies it (the token included, when one is required):

```text
claude mcp add --transport http --scope user oxide http://127.0.0.1:7878/mcp
```

This connects Claude Code straight to Oxide's HTTP address. Claude Code then finds Oxide only
if Oxide is already running when the session starts, and its tools aren't listed while Oxide is
closed. **Configure** sets up the bridge instead, which has neither limit. With a token, add
`--header "Authorization: Bearer <token>"` to the command.

## The token

With **Require a token** ticked, only clients that send the token can connect; a request
without it is refused (HTTP 401). The token is 64 random hexadecimal digits made by your
computer. It is shown in part in the settings: **Copy** next to the command copies the command
with the whole token.

**New token** makes another one and locks out every client using the old one at once. Press
**Reconfigure** on each Claude afterwards, and restart Claude Desktop. If a Claude tells you
Oxide turned its token away, that is why.

The token is a setting of the app, never an action. An agent can't read it, change it or turn
the requirement off.

## Ports

Oxide listens on port 7878 until you choose another, anywhere from 1024 to 65535. A new port
applies when you press [[Enter]] or click elsewhere in the settings, and the server restarts
on it at once.

Claude is configured for one port, so after changing it press **Reconfigure** on each Claude
(the row says *Configured for another port* until you do). If the port is taken by another
program (or another Oxide), the status bar says *MCP off: port 7878 is in use*: close the other
program and press **Retry**, or pick another port. Only programs on this computer can connect,
whatever the port.

## While Oxide is closed

Through the bridge, Claude lists Oxide's tools and prompts whether or not Oxide is running.
While Oxide is closed, a tool call answers *Oxide isn't running: start it, then try again*: an
ordinary tool error, so Claude tells you rather than treating the connection as broken. Start
Oxide and the next call goes through, in the same Claude session; nothing needs restarting.

Keep `oxide-mcp.exe` beside Oxide's program. The copy Claude runs sits in a `mcp-bridge` folder
there, so installing a new Oxide can replace the original while Claude Desktop is using the
copy. **Reconfigure** makes the copy again if it goes missing or is out of date.

## See also

- [[page:mcp-not-connecting]]: when Claude can't reach Oxide.
- [[page:ai-control]]: what Claude can and can't do once it is connected.
