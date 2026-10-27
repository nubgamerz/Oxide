# Automation

Automation moves a control over time by itself: the volume rising into a chorus, a pan
sweeping across, a delay's mix turned up for one phrase, a plug-in's knob turning. Each moving
control has a **lane** under its track, a line through points you draw or record by moving
the control while the song plays.

Lanes play the same live and in an export, at any buffer size, and a move lands on the audio
it was drawn over whatever the plug-ins' latency.

## What can be automated

A track (or the master) can have a lane for:

- **Volume** (−60 dB to +12 dB), **Pan** and **Mute**;
- each of its **sends'** levels (*Send → Reverb*);
- any parameter of its plug-ins (*Compressor: Threshold*), VST3 or built in. A few
  built-in settings have no lane (they would change the effect's delay, or rebuild it while
  playing): the effects' parameter tables say which ([[page:effects]]).

A mute lane takes over from the M button, but solo still wins. When a send or plug-in is
removed, its lanes go with it.

## Automate the volume

1. Right-click the track's header and point to **Add automation lane**.
2. Choose **Volume** (or **Pan**, **Mute**, a send, or a plug-in and then one of its
   parameters: type to filter a long list).
3. The lane opens under the track, starting at the control's current setting. Double-click
   in it to add a point; drag points up and down, and along the timeline.

::: shot automation
crop automation.lane track.header 6
highlight automation.lane.header "1" "The lane: its name, **On**, the value at the playhead, **×** to remove it."
highlight automation.point "2" "A point (selected: white). Drag it; [[Delete]] deletes it."
highlight automation.toggle "3" "Show or hide the track's lanes."
Guitar L's volume and pan lanes.
:::

The chart button on the header shows or hides a track's lanes (it appears once the track has
one); the header's right-click menu has **Show automation lanes** too. The master's lanes
open under its row at the top of the Multitrack.

## Drawing and editing points

| to | do |
|---|---|
| add a point | double-click the lane, or right-click > **Add point here** |
| move a point | drag it; its value shows beside the pointer |
| delete a point | click it and press [[Delete]], or right-click it > **Delete point (Del)** |
| shape the line to the next point | right-click the point > **Straight**, **Ease in (slow start)** or **Ease out (fast start)** |
| make a jump | put two points at the same time, one above the other |
| stop a lane playing | its **On** button, or right-click > **Switch lane off** (kept, but moves nothing) |
| remove a lane | **×** on its header, or right-click > **Remove lane** |

Points snap to the song's grid while **Snap** is on ([[page:multitrack#the-grid-and-snap]]).
Clicking an empty part of a lane moves the playhead there, as in the lanes above it. Points sit
in musical time: change the tempo and they stay on their beats.

While a lane plays, its control (the fader, the pan, the mute button) follows it and is drawn
greyed and outlined: moving it by hand does nothing in **Read**.

## Automation modes: Read, Touch, Latch and Write {#automation-modes}

Each track and the master has an **automation mode**: what moving its controls while the song
plays does to its lanes. Set it in the header's menu (*Read* ▾), with the mixer strip's
button (**R**, **T**, **L**, **W**: click for the next), or under **Mixer** in the inspector.

| mode | while playing |
|---|---|
| **Read** (the default) | Lanes play; moving a control does nothing to them. |
| **Touch** | Holding a control (the fader, pan, mute, or a knob in an open plug-in window) writes it; let go and the lane takes over again, gliding back to it. |
| **Latch** | Like Touch, but once moved a control keeps writing its last value until playback stops. |
| **Write** | Every lane of the track is written from the start of playback, from where its control is. |

::: shot automation-mixer
crop mixer.strip 6
highlight mixer.strip.automation_mode "1" "The mode button: **T**, Touch."
Guitar L's strip in Touch.
:::

### Write automation with a control

1. Set the track's mode to **Touch** (or **Latch**).
2. Play the song.
3. Move the fader, the pan, or a knob in the plug-in's window when you want it to change, and
   let go.
4. Stop. The pass becomes one undo step, *Write automation*.

While writing you hear the control, not the lane. A control that had no lane gets one. To
redo a pass, undo it ([[Ctrl+Z]]) and play again; to write only part of a song, play from
just before it in Touch.

## Over MCP

An agent automates with [[action:add_automation_lane]], [[action:add_automation_points]],
[[action:set_automation_point]], [[action:remove_automation_points]],
[[action:set_automation_lane]], [[action:remove_automation_lane]] and
[[action:set_automation_mode]]; [[action:get_automation_lane]] and
[[action:get_automation_value]] read them back. Writing with a control needs someone at it,
so agents draw points instead.

## See also

- [[page:mixer]]: the controls that lanes move.
- [[page:keyboard#mouse-the-mixer-and-automation]]: every gesture in one table.
