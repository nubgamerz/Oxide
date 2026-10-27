# Settings

**Options > Settings…** has a section for each part of Oxide, listed down its side: **Audio**,
**MIDI**, **Projects** and **AI control (MCP)**. Each row has the setting's name on the left
and, under its control, a line saying what it does. Audio's changes wait for **Apply** (the
list marks *Audio* while one waits); every other section's apply at once.

Settings are Oxide's own, not a project's: they are kept in `settings.json` beside the
program and apply to every project. F1 in a section opens its part of this page.

## Audio

The audio device and how the engine runs on it. Choosing them is explained step by step on
[[page:audio-device]]; in short:

| row | sets |
|---|---|
| **Driver type** | **WASAPI** (shares the device with other programs) or **ASIO** (talks to the interface directly, for the smallest buffers) ([[page:audio-device#pick-a-driver-wasapi-or-asio]]) |
| **Output device** | WASAPI: where the master plays (*System default*, or a device) |
| **ASIO driver** | ASIO: the driver that plays and records (in place of the two devices); a driver that can't start isn't listed |
| **Input device** | WASAPI: what tracks record from |
| **Sample rate** | the rate projects play at (*Device default*, or a rate the device offers); a project made at another rate is converted when it opens ([[page:audio-device#sample-rate]]) |
| **Buffer size** | samples per buffer (*The driver's default*, or a size): smaller is less delay when monitoring, more work for the CPU ([[page:audio-device#buffer-size]]) |
| **Processing block** | *Each buffer as it comes*, or *Gather into* *n* *samples*: tiny buffers gathered into a bigger block for heavy plug-ins, at the cost of the latency the line under it gives ([[page:audio-device#the-processing-block]]) |
| **Audio threads** | how many threads run independent tracks in parallel: *Auto* (one per physical core but the first), *Audio thread only*, or a number ([[page:audio-device#audio-threads]]) |
| **Record offset** | samples to move recordings earlier (negative: later), on top of the measured round trip, if a loopback test finds takes off ([[page:audio-device#record-offset]]) |

**Apply** restarts the device with these settings (the record offset and the threads apply
without a restart); it is greyed until something changed. Beside it, *Running:* names the
device that runs now, or *Not applied yet*.

::: shot settings-audio
crop settings 0
highlight settings.audio.apply "1" "**Apply**: restarts the device."
Settings > Audio.
:::

## MIDI

Keyboards and controllers. Changes apply at once.

| row | does |
|---|---|
| **Inputs** | lists the MIDI inputs Oxide opened (✔ *name*), or *None connected*. Every input connected when Oxide starts is opened; armed instrument tracks play and record from them, each track's **Input** picking the port and channel ([[page:recording#choosing-the-input]]) |
| **Reconnect MIDI devices** | opens every MIDI input again: after plugging a keyboard in, or when one stopped answering (also in the Options menu) |

More on MIDI devices: [[page:audio-device#midi-settings]].

## Projects

Backups and how Windows opens project files. Changes apply at once.

| row | sets |
|---|---|
| **Autosave** | *every* *n* *min* (5 at first, up to 120): a backup copy while the project has unsaved changes and playback is stopped; 0 turns it off. Only Save writes the project file ([[page:projects#autosave-and-backups]]) |
| **Backups kept** | how many backups each project keeps, the newest (20 at first) |
| **File type** | **Open .oxide files with Oxide**: makes Windows open `.oxide` files with Oxide and show its icon, for your account only |
| **Settings file** | where `settings.json` is: beside the program, so copying its folder takes the settings along |

::: shot settings-projects
crop settings 0
highlight settings.projects.autosave "1" "**Autosave**: how often."
highlight settings.projects.backups "2" "**Backups kept**."
Settings > Projects.
:::

## AI control (MCP)

Oxide runs an MCP server so AI agents such as Claude Code and Claude Desktop can operate it:
read the project, look at the screen, and change anything you can ([[page:mcp-reference]]
lists what they can do). It listens only on this computer. Changes apply at once.

| row | does |
|---|---|
| **Server** | **Enabled**: the server on or off |
| **Port** | the port it serves on, 1024 to 65535 (7878 at first); applies on [[Enter]] or when you click elsewhere |
| **Token** | **Require a token**: only clients that send it can connect (as *Authorization: Bearer* …); **New token** makes another, locking out clients with the old one. The token never shows in full here |
| **Status** | whether it runs (*MCP http://127.0.0.1:7878/mcp*), or why not (a port in use, with **Retry**) |
| **Command** | the `claude mcp add` line that connects Claude Code to it over HTTP while Oxide runs; **Copy** copies it, token and all |

::: shot settings-mcp
crop settings 0
highlight settings.mcp.server "1" "**Enabled**."
highlight settings.mcp.port "2" "**Port**."
highlight settings.mcp.token "3" "**Require a token**."
highlight settings.mcp.status "4" "**Status**."
highlight settings.mcp.connect "5" "**Connect Claude**: a row per client."
Settings > AI control (MCP), neither client configured yet.
:::

### Connect Claude

**Connect Claude**, under the rows, adds Oxide to Claude Code and Claude Desktop for you, a
row each, with whether it is there: *Not installed*, *Not configured*, *Configured (port
7878)*, *Configured for another port*, or what needs changing (an old token, an out-of-date
bridge, an entry made over HTTP: **Reconfigure** fixes each):

1. Click **Configure** on Claude Code's row, Claude Desktop's, or both.
2. For Claude Desktop, quit it fully and start it again (it reads its settings only when it
   starts). Claude Code picks it up in its next session.
3. Ask Claude something about your project ("what tracks are in my song?").

**Reconfigure** replaces Oxide's entry (after changing the port or the token), **Remove**
takes it out, and **Check again** reads Claude's settings again after you changed them
elsewhere. Both are connected through `oxide-mcp.exe`, a small bridge beside Oxide, so Claude
lists Oxide's tools even while Oxide is closed, and they work once it runs. The **Command**
row's line connects Claude Code directly instead, but only while Oxide is running when its
session starts.

If Claude can't reach Oxide, [[page:mcp-not-connecting]] goes through it step by step.

## See also

- [[page:audio-device]]: choosing the device, buffer and processing block.
- [[page:projects]]: autosave and backups.
- [[page:mcp-reference]]: every MCP action.
