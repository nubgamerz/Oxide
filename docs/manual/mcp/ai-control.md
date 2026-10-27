# AI control with Claude

Oxide runs an MCP server inside the app, so an AI agent such as Claude Code or Claude Desktop
can operate it the way you do: read the project, look at the window, and change what you can
change. You say what you want ("add a bass track under the guitars and pan it left"); Claude
makes the calls, and you watch the project change in the window.

Everything an agent can do is an *action*: a named operation with its inputs, its answer and
a safety class. The menu items, buttons and gestures of the app have actions behind them, and
a new feature gets its actions in the same change, so the list in [[page:mcp-reference]] is the
list of what Claude can do.

## What Claude can do in Oxide

- **Read the project.** [[action:get_project]] lists every track, clip, insert, send, marker and
  automation lane with its id; other reads give the tempo map, the MIDI in a clip as music
  ([[action:describe_midi_clip]]: key, chords, note names), a clip's waveform, a file's
  spectrum, the meters, the loudness and the CPU use.
- **Look at the screen.** [[action:screenshot]] shows the window as you see it, and
  [[action:get_view]] says what is selected and how far the timeline is zoomed.
- **Build and edit.** Tracks and folders, routing and sends, the mixer, plug-ins and their
  parameters, audio and MIDI clips and their notes, automation, tempo and markers: see the
  area pages, from [[page:mcp-tracks]] to [[page:mcp-markers]].
- **Play, record and export.** Start and stop the transport, set the loop, the punch range and
  the metronome, record, and export a mix with a loudness target ([[page:mcp-transport]],
  [[page:mcp-loudness]]).
- **Work in the Waveform view.** Open a file, edit it, run its effects rack, find and repair
  clicks, and print the result ([[page:mcp-editor]], [[page:mcp-diagnostics]],
  [[page:mcp-rack]]).
- **Answer from the manual.** [[action:search_manual]] finds the passage that answers a
  question, and [[action:open_manual]] shows its page to you.

An edit is an undo step like any other, so Edit > Undo takes back what Claude did
([[page:mcp-how-actions-work#undo]] says which actions aren't undo steps).

## What it can't do

- **Reach Oxide when it isn't running.** The tools stay listed in Claude, but a call answers
  that Oxide isn't running until you start it ([[page:mcp-connecting#while-oxide-is-closed]]).
- **Reach it from another computer.** The server listens only on this computer. You can also
  require a token, so only programs that have it can connect.
- **Read or change the token.** The token is a setting of the app, never an action: no tool
  shows it or turns it off ([[page:mcp-connecting#the-token]]).
- **Listen.** An agent reads meters, loudness, spectra and waveforms, and reads MIDI as
  music, but it doesn't hear the mix. Ask it to play something, and judge by ear.
- **Use what needs a person at the controls.** Writing automation by holding a fader, or
  turning a knob in a plug-in's own window, is yours to do. Claude adds automation points
  with [[action:add_automation_points]] and sets parameters with
  [[action:set_plugin_parameter]] instead. Plug-in editor windows aren't in screenshots.
- **Take back everything.** Saving a project, saving a file in the Waveform view and exporting
  overwrite files, and none of them is an undo step. Actions that delete or overwrite carry the
  *Destructive* badge, and it is worth saving a copy of a project before a long session
  ([[page:projects]]).

Whether Claude asks you before it runs a tool is a setting of Claude Code or Claude Desktop,
not something Oxide decides.

## Where to go next

1. [[page:mcp-connecting]]: connect Claude Code and Claude Desktop, and what to do when Oxide
   is closed. If Claude can't reach Oxide, [[page:mcp-not-connecting]] goes through it.
2. [[page:mcp-how-actions-work]]: ids, units, undo, and what the badges on each action mean.
3. [[page:mcp-workflows]]: jobs from start to end, each a few calls: building a song's tracks,
   writing a MIDI part, a mix pass, an export with a loudness target, recording takes and the
   Waveform view's rack.
4. [[page:mcp-prompts]]: the guides Claude can offer you.
5. [[page:mcp-reference]]: every action by area, with its inputs, its answer and examples.
