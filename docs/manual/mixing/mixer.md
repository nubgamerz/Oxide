# The mixer

The mixer shows every track as a channel strip, side by side, with the master's strip at the
right. Each strip has the track's plug-ins, its sends, its pan, its fader and meter, and its
mute, solo and arm buttons. Everything on a strip is also in the track's header and in the
inspector: use whichever is in front of you.

Show or hide the mixer with the **Mixer** button in the transport bar, View > **Mixer (M)**, or
[[M]]. It opens as a pane under the Multitrack; drag its tab to put it anywhere else
([[page:main-window#the-panes-and-the-dock]]).

::: shot mixer
crop pane.mixer 0
highlight mixer.strip.name "1" "The track's name: click to select it, drag sideways to reorder."
highlight mixer.strip.inserts "2" "Inserts: the track's plug-ins, top to bottom."
highlight mixer.strip.sends "3" "Sends: a bar per send, its level as the fill."
highlight mixer.strip.pan "4" "Pan."
highlight mixer.strip.fader "5" "The fader."
highlight mixer.strip.meter "6" "The meter."
highlight mixer.strip.automation_mode "7" "M, S, ● (arm) and the automation mode."
highlight mixer.strip.output "8" "Where the track goes."
highlight mixer.master "9" "The master."
The mixer docked under the Multitrack, Guitar L selected (outlined).
:::

## A channel strip

From the top:

- **The name.** Click it to select the track (the inspector follows). Drag it sideways to move
  the track: the Multitrack's order changes too.
- **Inserts.** The track's plug-ins, in the order the sound goes through them. Click a name to
  open or close its window; a bypassed plug-in is struck through. **+** adds one. See
  [[page:#inserts-adding-plug-ins]].
- **Sends.** One bar per send, named after the bus it feeds; **PRE** on it means pre-fader.
  **+ send** adds one. See [[page:#sends-and-buses]].
- **Pan**, a slider: left to right. Double-click it to centre it. The value under it (*C*,
  *L30*, *R20*) can be clicked and typed.
- **The fader**, from −60 dB to +12 dB, with the meter beside it. Double-click the fader for
  0 dB. Beside the meter, the top number is the peak level now (red from −0.1 dB) and the one
  under it is the fader's level: click it and type a value (*-6*, *-6.5 dB*, *+3*).
- **M** mutes the track, **S** solos it, and **●** arms it for recording (audio and instrument
  tracks only, [[page:recording#arming-a-track]]).
- **The automation mode**: **R**, **T**, **L** or **W** (Read, Touch, Latch, Write). Click it
  for the next mode ([[page:automation#automation-modes]]).
- **→ Output**: where the track's sound goes (*Master*, a folder or bus, *Out 3+4*). Change
  it in the inspector ([[page:#where-a-track-goes-its-output]]).

The selected track's strip is outlined in blue. Folders and buses are drawn darker than
audio and instrument tracks.

### Faders, pan and levels

A track's **volume** (its fader) and **pan** are applied after its plug-ins, so a plug-in
always gets the same level whatever the fader says. The same controls are on the track's
header (the knobs, on taller rows) and in the inspector's **Mixer** section, where **Volume**
and **Pan** take typed values too.

Pan on a stereo track is a balance: it turns one side down rather than moving the sound
across.

While an automation lane plays a control, the control follows the lane and is drawn greyed
and outlined: move it and nothing changes, unless the track is in a write mode
([[page:automation]]).

### Meters

Each meter shows the track's peak level after its fader, left and right, with a line holding
the highest recent peak. The scale runs from −60 dB at the bottom to +6 dB at the top; the
number beside it turns red from −0.1 dB, just under full scale.

A track you are monitoring shows its input on the meter, after its plug-ins and fader
([[page:recording#monitoring-while-playing-and-recording]]). The master's strip also shows
the mix's integrated loudness ([[page:exporting#the-loudness-meter]]).

### Mute, solo and switching a track off

- **Mute** (M) silences the track and its sends.
- **Solo** (S) plays only the soloed tracks. It is solo *in place*: a soloed track keeps
  the folders it sits in and the buses it sends to, so you hear its reverb; a soloed folder
  keeps its tracks; and a soloed bus plays with exactly what feeds it (the guitar's reverb
  without the dry guitar).
- **Switching a track off**, with the power button on its header (or right-click the header >
  **Disable track**), takes it out of the mix altogether: unlike a mute, its plug-ins stop
  and cost no CPU. Its settings are kept; switch it back on with the same button or **Enable
  track**.

## Inserts: adding plug-ins

A track's **inserts** are its plug-ins, built-in effects and VST3 plug-ins alike, run in
order: the first hears the track's audio (or MIDI, for an instrument) first, the last feeds
the fader. They are listed on the mixer strip, in the inspector's **Inserts** section and,
on tall enough rows, in the track header's effects rack.

To add one:

1. Click **+** in the strip's inserts (or **+ Add plug-in…** in the inspector, or an empty
   slot in the header's rack).
2. In the plug-in browser, type part of its name or maker's name to filter the list.
3. Click the plug-in. It loads (a bar says what is loading) and goes at the end of the chain,
   or into the slot you clicked.

::: shot browser
crop browser.search browser.rescan 60
highlight browser.search "1" "Type to filter by name or maker."
highlight browser.rescan "2" "**Rescan** looks for plug-ins installed since Oxide started."
The top of the plug-in browser. Oxide's own effects are listed first, under *Built in*,
then the VST3 plug-ins by maker.
:::

With a single plug-in left in the list, [[Enter]] loads it; [[Esc]] closes the browser.
An instrument plug-in on an instrument track is put first in the chain, so the MIDI reaches
it. A plug-in that won't load, or isn't in the list, is covered on
[[page:plugin-problems]].

Oxide's built-in effects (equalisers, compressors, reverbs, delays, noise reduction) are
described on [[page:effects]].

### Plug-in windows

Click a plug-in's name (on a strip, in the inspector or in the header's rack) to open its
window, and click it again to close it. A VST3 plug-in shows its own editor in a window of
its own; a built-in effect opens a small editor titled with the effect and the track
(*Parametric EQ · Guitar L*). The inspector's tooltip on a plug-in gives its latency in
samples.

Changing a plug-in's settings is part of the project: it is saved with it and undone with
[[Ctrl+Z]].

### Bypass, reorder, copy and remove

| to | do |
|---|---|
| bypass a plug-in | its box in the inspector, its power button in the header's rack, or right-click it > **Bypass** (**Enable**, or **Turn on**, brings it back) |
| bypass the whole chain | the rack's power button, in the header beside *fx* |
| move it up or down | **▲** / **▼** in the inspector, or drag its number in the header's rack to another slot |
| move it to another track | drag its number in the header's rack onto the other track's rack |
| copy it to another track | **Ctrl**+drag its number, or right-click > **Copy**, then right-click an empty slot (or **+**) > **Paste** |
| put a copy after another plug-in | right-click that plug-in > **Paste** *name* **after** |
| swap it for another | right-click in the header's rack > **Replace…** |
| remove it | **✕** in the inspector, or right-click > **Remove** |

A copy keeps the plug-in's settings. A bypassed plug-in passes the sound through unchanged
but keeps its delay (latency), so the track stays in time.

### Multi-out instruments

Some instruments have more outputs than their main pair: a sampler's extra outputs, a drum
sampler's kick and snare, a synth's parts. To mix them on faders of their own:

1. Right-click the instrument (on a strip, in the inspector or in the header's rack) and
   choose **Create multi-out rack in a submix…** (it is only offered when the plug-in has
   extra outputs).
2. Tick the outputs you want (**All**, **None**) and click **Create** *n* **tracks**.

Each output gets an audio track in a new folder under the instrument's track, with its own
fader, pan, plug-ins and sends. Inside the plug-in, send each sound to its output. The main
pair stays on the instrument's own track (muting that track leaves the outputs playing).
**Remove multi-out rack**, in the same menu, takes the tracks away again.

## Sends and buses

A **send** feeds a copy of a track's sound to a **bus** (a return track) or a folder, at a
level of its own. Use it to share one reverb or delay between several tracks: put the reverb
on a bus, then send each track to it.

### Add a send

1. Right-click the track's header and point to **Add send** (or click **+ send** on its strip,
   **+ Add send** in the inspector's **Sends**, or Track > **Add send from selected track**).
2. Choose the bus or folder to send to, or **New bus** to make a new bus and send to it in
   one step.

A new send is at 0 dB and post-fader. Only buses and folders that can't make a loop are
offered (a track can't send to its own folder, for one).

### Send level, pre and post

On a strip, a send is a bar whose fill is its level, from −60 dB to +12 dB:

- drag it sideways to change the level, double-click it for 0 dB;
- right-click it for **Pre-fader**, **Mute** and **Remove**.

In the inspector's **Sends** section each send has its bus (change it there), **Pre**, **M**
(mute), **✕** (remove) and a level slider.

- **Post-fader** (the default): the send follows the track's fader and pan, so turning the
  track down turns its reverb down too.
- **Pre-fader** (**Pre** lit, **PRE** on the bar): the send is taken before the fader and pan,
  so the fader doesn't change it (a headphone mix, or a reverb you keep while fading the dry
  sound).

Muting a track also mutes its sends. Each send keeps its own delay compensation, so a bus with
a slow plug-in stays in time with the dry tracks.

### Buses

Add a bus with **+ Track** > **Bus (return)** or Track > **Add bus (return)**, or with a
send's **New bus**. A bus has no clips: it plays what its sends (and any track whose output
is set to it) feed it, through its own plug-ins, fader and sends.

## Folders

A **folder** holds tracks: their sound is mixed into the folder, which has its own plug-ins,
fader, mute and solo, like a submix. Put a compressor on a *Guitars* folder to treat every
guitar together, or pull the whole drum kit down with one fader. Folders can hold folders.
Making folders and moving tracks into them is on [[page:multitrack#folders-buses-and-the-master]].

## Where a track goes: its output

Select the track and open **Output** under **Routing** in the inspector:

| choice | the track goes to |
|---|---|
| **Parent** (*Folder: …* or *Master*) | its folder, or the master when it is in none (the default) |
| **Master** | the master, even from inside a folder |
| **Bus:** *name* | a bus or folder (one that can't make a loop) |
| **Out 1+2**, **Out 3+4** … **(direct)** | a pair of the audio device's outputs, past the master |

A track sent straight to the device's outputs skips the master: its plug-ins, fader, mute and
loudness meter. Use it for a click or a cue mix on another pair of outputs.

::: shot automation
crop inspector.routing inspector.mixer 8
highlight inspector.output "1" "Output: where the track goes."
The inspector's Routing section with Guitar L selected.
:::

## The master

The master is the mix of everything, and what you hear and export. Its strip is the right-most
in the mixer (*MASTER*); in the Multitrack, it is the row at the top of the track list. Click
its name, or **Master** beside + Track, to select it: the inspector then shows its
**Output** (the pair of device outputs it plays on), **Loudness** (the meter,
[[page:exporting#the-loudness-meter]]), **Mixer** (volume, pan, **Mute**, automation) and
**Inserts**.

- **Plug-ins on the master** process the whole mix: a mastering EQ, a limiter (a mastering
  limiter plug-in, or the built-in [[effect:limiter]]). Add them with the strip's **+**, the inspector's
  **+ Add plug-in…** or the master row's rack.
- **Mute** on the master silences everything, the Waveform view's preview included. A
  mixdown to a new file needs the master unmuted ([[page:rendering#mix-down-to-a-new-file]]).
- The master's strip shows its integrated loudness under the buttons, and **→ Out 1+2** its
  outputs.

::: shot master
crop pane.multitrack 0
highlight master.mute "1" "The master's mute."
highlight master.rack "2" "Its plug-ins: here a Limiter."
The master's row selected in the Multitrack, tall enough for its rack, with a volume lane.
:::

## Over MCP

An agent mixes with [[action:set_track_mixer]] (volume, pan, mute, solo),
[[action:set_track_output]], [[action:add_send]], [[action:add_send_to_new_bus]],
[[action:set_send]], [[action:remove_send]], [[action:add_insert]], [[action:set_insert]]
(bypass), [[action:move_insert]], [[action:remove_insert]], [[action:copy_insert]],
[[action:set_inserts_bypassed]], [[action:set_plugin_editor]], [[action:list_plugins]],
[[action:rescan_plugins]], [[action:create_multi_out_rack]], [[action:set_master]] and
[[action:set_track_enabled]]. [[action:get_meters]] reads the meters.

## See also

- [[page:automation]]: moving faders, pans and plug-in settings over time.
- [[page:effects]]: Oxide's own effects.
- [[page:choppy-audio#find-what-is-heavy-the-cpu-panel]]: which plug-in costs the most.
