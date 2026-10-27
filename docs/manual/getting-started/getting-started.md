# Getting started

This page takes you from installing Oxide to your first recorded take. The two pages after it
go further: [[page:main-window]] names every part of the window, and [[page:audio-device]]
explains the audio settings.

## Installing and updating

1. Run `Oxide-Setup-<version>.exe`. It installs for your Windows account only, so it never
   asks for an administrator.
2. Keep the suggested folder (a folder named `Oxide` on one of your drives, or
   `%LOCALAPPDATA%\Programs\Oxide`), or pick another folder that is yours to write to. Oxide keeps its
   settings beside the program.
3. Tick the options you want: a desktop shortcut, and opening `.oxide` projects with Oxide when
   you double-click them in Explorer. A Start menu entry is always made.

**Updating:** run a newer installer. It finds the installed folder by itself and replaces only
the program and this manual. What Oxide keeps beside itself stays as it is: your settings
(`settings.json`), the Waveform view's effects rack (`rack.json`), your project templates
(`Templates`), rack presets (`RackPresets`) and the copy of the Claude bridge (`mcp-bridge`).
If Oxide is running, the installer asks you to close it first, so save your work. An older
installer refuses to replace a newer Oxide.

**Which version you have:** the version is at the right end of the status bar; click it, or use
Help > About Oxide…, for the version and the build.

**Uninstalling:** Windows Settings > Apps > Installed apps > Oxide > Uninstall (or
`unins000.exe` in Oxide's folder). It asks whether to keep your settings, effects rack,
templates and rack presets; keeping them is the default. Your projects are never in Oxide's
folder unless you saved them there, and the uninstaller never removes them.

## The start screen

When no project is open, Oxide shows the start screen: a new project on the left, the projects
you opened lately on the right. File > New project… ([[Ctrl+N]]) shows it again over the
project you have open; its **Cancel** button closes it.

::: shot start
crop start.new start.recent 36
highlight start.template "1" "Template: start from a saved setup instead of an empty project."
highlight start.folder "2" "In folder: where the project's folder is made."
highlight start.create "3" "Create project makes the folder and opens the project."
highlight start.open "4" "Open project… picks any `.oxide` file."
highlight start.recent "5" "Recent projects: click one to open it."
The start screen (File > New project…).
:::

### Make a new project

1. Type a **Name**.
2. Leave **Template** on *None (empty)*, or pick one of your templates (below).
3. Check **In folder**: the folder the project goes in. **Browse…** picks it; the line under it
   says the exact folder that will be made, such as `D:\Music\My song`.
4. Press **Create project**.

Oxide makes a folder with the project's name, saves the project file inside it and opens it.
[[page:#the-project-folder|The project folder]] below says what goes in it.

### Start from a template

A template is a project's setup without its recordings: its tracks, their routing, plug-ins and
settings. Save one from any project with File > Save as template…, then pick it in the start
screen's **Template** list. File > Templates… lists your templates, to rename or delete them.

### Open a project

- Click a project under **Recent projects**. The one already open says *Open now*; one
  made from a template says which.
- A project that was moved or deleted says *Missing*; **Remove from the list** takes it off
  the list (nothing is deleted). Right-click a row for **Open**, **Show in Explorer** and
  **Remove from the list**.
- **Open project…** (or File > Open…, [[Ctrl+O]]) opens any `.oxide` file. File > Open recent
  lists the same projects as the start screen.

A project opens behind a loading bar: its audio files are read first, then its plug-ins load
one at a time. **Cancel** on the bar goes back to the project you had open.
[[page:slow-open]] says what the bar is doing.

If the project you are leaving has changes that aren't saved, Oxide asks first: *Save*, *Don't
save* or *Cancel*.

An AI agent does the same with [[action:new_project]], [[action:open_project]] and
[[action:list_recent_projects]].

## The project folder

A project is a folder. Everything it uses lives inside, so you can copy or move the whole
folder to another drive or computer and it opens there:

| in the folder | what it holds |
|---|---|
| `<Name>.oxide` | The project file: tracks, clips, routing, plug-in settings, automation. |
| `Recorded` | Every take you record, as WAV files. |
| `Imported` | A copy of every file you import or drop in, so the original is never changed. |
| `Rendered` | Rendered tracks and mixdowns. |
| `Exported` | Exports, unless you pick another place when you export. |
| `Backups` | Copies of the project file saved as you work, every 5 minutes by default (File > Open backup…). |
| `Temp` | Temporary files, which would otherwise go to Windows' temp folder. |

Save with File > Save ([[Ctrl+S]]). File > Save as… saves the project under a new name and
folder.

## A first recording in five steps

This assumes your audio interface is plugged in. Recording has a page of its own:
[[page:recording]].

1. **Choose the audio device.** Open Options > Settings…, choose **Audio** on the left, pick
   your interface's driver and device, and press **Apply**. [[page:audio-device]] explains each
   setting.
2. **Add a track.** Click **+ Track** above the track list and choose **Audio track** (or press
   [[Ctrl+T]]).
3. **Pick its input.** With the track selected, open the inspector (the **Inspector** button at
   the right of the transport bar) and set **Input** under *Routing* to the input your guitar
   or microphone is plugged into, such as *In 1*.
4. **Arm it.** Click the track's **R** button. With **Monitor** on *Auto (when armed)*, you
   now hear the input through the track and its plug-ins.
5. **Record.** Click the round Record button in the transport bar, or press [[R]]. Play, then
   press [[Space]] to stop. The take appears as a clip where you recorded it, and its file is
   in the project's `Recorded` folder.

::: shot recording
crop inspector.input inspector.monitor 30
highlight inspector.input "1" "Input: what the track records."
highlight inspector.monitor "2" "Monitor: when you hear the input."
Step 3: the inspector's Routing, with Vocal selected.
:::

::: shot recording
crop track.header 30
highlight track.arm "3" "R arms the track for recording."
highlight track.monitor "4" "The headphones button: input monitoring."
Step 4: Vocal armed and recording; its take grows on the timeline as it comes in.
:::

## See also

- [[page:main-window]]: what each part of the window is.
- [[page:audio-device]]: drivers, buffer size and the processing block.
- [[page:recording#monitoring-while-playing-and-recording]]: hearing yourself through your amp
  sims.
