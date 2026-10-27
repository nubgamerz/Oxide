# Projects: saving, backups and templates

A project is a folder holding the project file and everything it plays
([[page:getting-started#the-project-folder]]). This page covers saving it, the backups Oxide
keeps as you work, starting projects from templates, and moving a project to another place.

## Saving

**File > Save** ([[Ctrl+S]]) writes the project file, `<Name>.oxide`, in its folder. A bar
shows *Saving* while each plug-in's settings are read (one at a time, so the window keeps
drawing) and the file is written in the background. A project that has never been saved asks
where first, as Save as… does.

The file holds everything about the song: its tracks, clips, routing, mixer, sends,
automation, tempo and time signature, markers, grid, loudness target and every plug-in's
settings (an amp simulator's, a mastering module's) inside it. The audio stays in the folder's
`Recorded` and `Imported` files. The zoom and scroll of the view are not saved.

The right end of the menu bar shows the project's file; *(not saved)* after its name means it
has none yet.

### Save as

**File > Save as…** saves the project under another name or in another folder. Saved into
another folder, the project's files (its recordings, imported files, renders) are copied
there with it, so the copy is a complete project of its own; the old one is left as it was.

### Before closing

Closing Oxide, opening another project or starting a new one with unsaved changes asks
**Save the changes to** *Song* **before closing?** (or *before opening …*), listing any files
edited in the Waveform view too: **Save** saves and goes on, **Don't save** goes on without,
**Cancel** stays.

## Autosave and backups

While a project has unsaved changes, Oxide copies it into its `Backups` folder every 5
minutes (*Name date time*.oxide), keeping the newest 20. The project file itself is only
written when you save. A backup waits while the song plays or records (reading a plug-in's
settings could interrupt its sound) and is made once you stop. Change the minutes (0 turns
autosave off) and how many to keep in Options > Settings… > **Projects**
([[page:settings#projects]]).

### Restore a backup

1. Choose File > **Open backup…**. It lists the project's backups, newest first, with when
   each was made and its size.
2. Click **Open** beside the one you want. If the project has unsaved changes, Oxide asks
   whether to save them first.
3. The backup is now the open project. Listen, then **Save** to keep it as the project (or
   open the project again to go back).

::: shot backups
crop backups 8
File > Open backup… with two backups of an example project.
:::

If Oxide closed without saving (a crash, a power cut), opening the project finds the newer
backup and asks **A newer backup**: **Open the backup**, or **Keep the saved project**.

## Templates

A template is a project's setup without its recordings: its tracks, folders, buses, routing,
sends, mixer and plug-ins with their settings (an amp sim's tone too), its automation lanes
without their moves, the tempo, time signature, loudness target and markers. Start every song
from your usual tracking setup, or your mastering chain.

### Save as template

File > **Save as template…**, type a **Name**, **Save**. A name that is taken asks to
**Replace** it.

### Start a project from a template

Pick it in the start screen's **Template** list (File > **New project…**,
[[page:getting-started#start-from-a-template]]): the new project starts as a copy of it.

::: shot templates
crop start.new 8
highlight start.template "1" "The template the new project starts from."
The start screen's New project column with a template picked.
:::

### Rename or delete templates

File > **Templates…** lists them, each with **Rename…** and **Delete**.

Templates live in a `Templates` folder beside the program, for every project.

## Moving or copying a project

Close it, then move or copy its whole folder (in Explorer) to another drive or computer: it
opens from there, since everything it uses is inside. Open it with File > **Open…**
([[Ctrl+O]]), from **Open recent**, from the start screen, or by double-clicking the `.oxide`
file once Oxide is set to open them ([[page:settings#projects]]).

A project that uses a plug-in not installed on the other computer still opens: the plug-in's
slot keeps its settings, so it comes back when the plug-in is installed
([[page:plugin-problems#a-plug-in-in-a-project-didnt-load]]).

## Over MCP

[[action:save_project]], [[action:open_project]], [[action:new_project]],
[[action:list_recent_projects]], [[action:list_backups]], [[action:save_backup]],
[[action:open_backup]], [[action:save_template]], [[action:list_templates]],
[[action:rename_template]], [[action:delete_template]] and [[action:get_unsaved_changes]].

## See also

- [[page:getting-started]]: making a project.
- [[page:slow-open]]: what the loading bar says.
