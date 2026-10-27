# A plug-in is missing or won't load

Oxide hosts VST3 plug-ins. This page covers a plug-in that doesn't appear in the browser, and
one in a project that didn't load.

## A plug-in isn't in the browser

Oxide finds VST3 plug-ins in Windows' standard VST3 folders:

- `C:\Program Files\Common Files\VST3`
- `C:\Program Files (x86)\Common Files\VST3`

and in folders inside them. Only VST3 plug-ins are listed (`.vst3`), not VST2 (`.dll`) or CLAP.

1. Check the plug-in's installer put its VST3 version in one of those folders (some ask, and
   default to VST2 only).
2. Installed it while Oxide was running? Choose Options > **Rescan plug-ins**, or press
   **Rescan** in the plug-in browser.
3. In the browser, type part of its name or its maker's: the list is filtered as you type.
   Oxide's own effects are listed first, under *Built in*, then the VST3 plug-ins by maker.

## A plug-in in a project didn't load

When a project opens, each plug-in is loaded with its saved settings. One that can't load (it
was uninstalled, its licence ran out, it fails to start) keeps its place:

- In the track header's rack, its name is **struck through**. Point at it: the tooltip says why
  it didn't load, when the plug-in said, and that *its settings are kept with the slot; remove
  it to forget them*.
- In the inspector's **Inserts**, its name is red and struck through, with *Failed to load*.
- The track plays **without it**: the plug-ins around it still run.

::: shot rack-restored
crop waveform.rack 8
highlight waveform.rack "1" "A plug-in that didn't load keeps its slot and its settings, struck through."
The Waveform view's effects rack, with one plug-in no longer installed.
:::

To get it back, reinstall (or re-license) the plug-in, then reopen the project: its saved
settings load into it. To replace it, right-click its name > **Replace…** and choose another;
to drop it, **Remove**.

Licences and activation belong to the plug-in: if it asks to be activated, use its own window
or its maker's app. Some plug-ins report a different latency once
running; Oxide reads it after they start, so delay compensation stays right.

## A plug-in crashes Oxide

Plug-ins run inside Oxide, so one that crashes takes Oxide with it. Oxide saves a copy of your
project in its `Backups` folder as you work (every 5 minutes unless Settings > Projects >
**Autosave** says otherwise): File > Open backup… opens one. Bypass or remove
the plug-in that crashed (open the backup, then right-click its slot) and tell its maker.

## Over MCP

[[action:list_plugins]] lists what Oxide found, [[action:rescan_plugins]] scans again, and
[[action:get_project]] shows each insert and whether it loaded.

## See also

- [[page:slow-open]]: plug-ins loading when a project opens.
- [[page:choppy-audio]]: a plug-in too heavy for the computer.
