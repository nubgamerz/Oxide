# Oxide manual

Oxide records, edits, mixes and masters in one program. You record guitar, voice or MIDI
instruments through your plug-ins, arrange and edit takes in the Multitrack, write MIDI parts in
the piano roll, mix with plug-ins and Oxide's own effects, then finish a file in the Waveform
view and export it.

One idea runs through all of it: **what you export is what you heard**. An export runs through
the same engine, the same plug-in instances and the same block size as playback, so a bounce
never surprises you.

## Opening the manual

In Oxide, Help > Manual opens this page in your web browser. [[F1]] opens the page about the
part of Oxide you are pointing at: with the pointer over the piano roll it opens
[[page:piano-roll]], in Options > Settings… the page's section for the settings shown, over the
mixer [[page:mixer]], over a built-in effect's editor that effect's section. With the pointer on
none of them, it opens the page for the window that is open, or for the view you are in.

The manual is in the `manual` folder beside `Oxide.exe`, and opens from there without a
network. Installing a newer Oxide replaces it with that version's.

An AI agent connected to Oxide can read the manual too: [[action:search_manual]] finds the
passages that answer a question, and [[action:open_manual]] shows you a page.

## How this manual is organised

The chapters are down the left side; the headings of the page you are reading are down the
right (on a wide enough window). At the bottom of every page, *Previous* and *Next* walk
through the manual in order.

- **Getting started** installs Oxide, makes a first project and records a first take;
  **Recording**, **Editing** (the Multitrack and the piano roll) follow the work in order.
- **Mixing** covers [[page:mixer|the mixer]] and [[page:automation|automation]], and **Built-in
  effects** [[page:effects|every effect Oxide brings]], with its parameters.
- **The Waveform view** finishes a single file: editing, repair, noise reduction, tags.
  **Exporting** writes the mix ([[page:exporting|formats and loudness]], [[page:rendering|track
  renders and mixdowns]]); **Projects** and **Settings** cover saving, backups, templates and
  every setting.
- **Troubleshooting** starts from what you hear or see going wrong, such as
  [[page:choppy-audio|crackles and a high DSP load]], [[page:no-sound|no sound]] or
  [[page:mcp-not-connecting|Claude not reaching Oxide]].
- **Reference** has every [[page:keyboard|key and mouse gesture]] and a
  [[page:glossary]] of the words used.
- **AI control (MCP)** lists [[page:mcp-reference|every action]] an AI agent such as Claude
  can call to operate Oxide, from [[action:get_project]] to [[action:export_mixdown]].

Pictures of the program have the parts they talk about outlined, with numbers that match the
list under the picture.

## Searching

Type in the box at the top of any page, or press [[/]] or [[Ctrl+K]] to jump there. Results
appear as you type: pages whose title matches come first, then sections whose heading
matches, then sections that mention your words. Plain questions work too: common words such
as "how", "do" and "the" are left out, so "how do I stop the crackling" looks for "stop" and
"crackling", and sections with both come first.

- [[Up]] and [[Down]] move through the results; [[Enter]] opens the one marked.
- [[Esc]] closes the results, and pressing it again empties the box.

The page you open has your words highlighted, with a bar at the top saying how many there are;
its *Clear* button takes the highlights away.

## Keys and names

Keys look like [[Ctrl+Shift+Z]]. Menu items are written as a path: Options > Settings… means
the Settings… item in the Options menu. Names of buttons, settings and windows are written as
the program shows them.
