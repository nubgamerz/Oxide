# Recording

To record, you give a track an input, arm it, and press Record. This page covers each step,
how to hear yourself through your amp sims while you play and record, and the count-in, the
metronome, punch and loop recording.

## Audio and instrument tracks

Two kinds of track record:

- An **audio track** records sound from an input of your audio interface: a guitar, a
  microphone. Its takes are audio clips.
- An **instrument track** records MIDI from a keyboard or controller, and plays it through the
  instrument plug-in in its first insert (a synth, a sampler, a drum plug-in). Its takes are MIDI
  clips.

Add one with **+ Track** above the track list (**Audio track** or **Instrument track**), the
Track menu, or right-click the empty space under the tracks. [[Ctrl+T]] adds an audio track, and
double-clicking the empty space under the tracks does too. A new track goes into the selected
folder, or into the folder of the selected track.

Folders and buses never record: they carry other tracks' sound
([[page:multitrack#folders-buses-and-the-master]]).

## Choosing the input

Select the track (click its header) and look under **Routing** in the inspector:

- **Input** on an audio track is *No input*, a single input (*In 1*, *In 2*, …) for a mono
  source such as a guitar or a microphone, or a pair (*In 1+2*, …) for a stereo source. A new
  audio track starts on *In 1*. With no inputs on the device it says *The audio device has no
  inputs*: choose an input device in [[page:audio-device]].
- **Input** on an instrument track is *No input*, *All MIDI inputs*, or one MIDI keyboard by
  name; **MIDI channel** is *Omni* (every channel) or 1 to 16. A new instrument track listens
  to all MIDI inputs, on every channel. A keyboard that isn't plugged in now reads
  *— not connected*.

A mono input is recorded as a mono file and heard in the middle of both speakers.

::: shot recording
crop inspector.routing inspector.monitor 12
highlight inspector.input "1" "Input: what the track records."
highlight inspector.monitor "2" "Monitor: when you hear the input."
The inspector with Vocal selected.
:::

## Arming a track

A track's **R** button arms it: Record then records it. Arm as many tracks as you like, and
Record takes them all at once, each into its own file. The mixer's **●** button does the same.

- An armed audio track needs an input; an armed instrument track records its MIDI input.
- New instrument tracks are **armed as they are made**, so your keyboard plays them at once.
  Click R to disarm one.
- With nothing armed, Record says *Can't record: arm an audio track with an audio input, or an
  instrument track, to record*.
- Arming is read when recording starts: a track armed during a take joins the next one.

::: shot recording
crop track.header 4
highlight track.arm "1" "R: armed (red)."
highlight track.monitor "2" "Input monitoring: Auto, On or Off."
Vocal's header, armed. Beside R are S (solo) and M (mute); at the right, the power button.
:::

## Monitoring while playing and recording

**Monitoring** means hearing your input through the track as you play: the guitar through the
track's amp sim, your voice through its effects. Each audio and instrument track has a
monitoring setting with three modes:

| mode | the headphones button | you hear the input |
|---|---|---|
| **Auto (when armed)** | grey, with a blue icon | while the track is armed |
| **Always on** | lit blue | always, armed or not |
| **Off** | dim, struck through | never |

::: shot header
crop 2 325 250 395
highlight track.monitor "1" "Guitar L: Always on (lit blue)."
The headphones buttons: Guitar L *Always on*, Guitar R and Keys on *Auto*, Vocal *Off* (struck
through).
:::

To monitor while you play:

1. Select the track and set its **Input** (above).
2. Leave **Monitor** on *Auto (when armed)* and click **R** to arm the track.
3. Play: you hear yourself through the track's plug-ins, whether Oxide is stopped, playing
   or recording.

Click the headphones button on the track's header to step through the modes (Auto, then On,
then Off), or choose one in the inspector's **Monitor** list. *Always on* is for a track you
want to hear without arming it, such as an amp sim you practise through. *Off* is for when you
monitor elsewhere, through your interface's direct monitoring, so you don't hear yourself
twice.

### Monitoring through amp sims and effects

The input runs through the whole track: its plug-ins in order, then its fader, pan and sends,
to the master. So an amp-sim plug-in, or any effect on the track, is what you hear. While
playing, the track's existing clips play along with the input.

What is **recorded** is always the dry input, before any plug-in. Change the amp sim's
settings after recording and the take plays through the new sound.

The track's meter shows the input only while it is monitored, and after its plug-ins and
fader. With Monitor off the meter stays still, but the input is still recorded.

### Latency while monitoring

What you hear is late by the device's round trip plus the buffer: the smaller the
[[page:audio-device#buffer-size|buffer size]], the closer to instant. Two things add to it:

- **The processing block**, when it gathers, adds the block less one buffer
  ([[page:audio-device#the-processing-block]]).
- **Plug-in latency anywhere in the project.** Oxide keeps every track in time by delaying all
  of them to match the slowest plug-in chain (the *PDC* in the status bar). A look-ahead
  limiter on the master, or on any other track, makes your monitoring that much later too
  (a look-ahead limiter can add hundreds of samples). Bypassing it isn't enough, since a bypassed plug-in
  keeps its delay: while tracking, remove such a plug-in, or switch off the track that holds
  it (its power button), and bring it back to mix.

## Recording a take

1. Arm the tracks to record and put the playhead where the take should start: click the ruler,
   or press [[Home]] for the start.
2. Press the Record button or [[R]]. Oxide plays from the playhead and records the armed
   tracks. The Record button is lit, and each take is drawn as it comes in.
3. Press [[Space]], the Stop button, or Record again to stop.

The takes become clips where they were recorded, in one undo step named *Record*. Recording
always starts at the playhead: a time selection doesn't limit it (use
[[page:#punch-recording|punch]] for that).

You can also press Record **while playing**: recording starts at once, where the playhead is,
with no count-in. Stopping stops both playback and recording.

While recording, the playhead, the loop, the punch range and the tempo can't be changed.

A take recorded over an existing clip is added as a new clip on top; **both play**, since
overlapping clips on a track are mixed. Delete or trim the old one if the new take replaces
it ([[page:multitrack#clips]]).

::: shot recording
crop arrange.ruler track.header 4
highlight track.header "1" "Vocal: armed, its take growing as it records."
A take growing on Vocal as it records.
:::

## The count-in

A count-in plays the metronome for one or two bars before recording starts, so you can come in
on time.

1. Right-click the Record button.
2. Choose **1 bar** or **2 bars** (**Off** turns it off).
3. Press Record. The click plays the count-in, then recording starts at the playhead.

::: shot record-menu
crop transport.record_menu 10
highlight transport.record_menu
The Record button's right-click menu: Count-in Off, 1 bar or 2 bars.
:::

- The count-in clicks even when the metronome is off.
- It plays only when you record from a stop; pressing Record while playing starts at once.
- The Record button shows a small yellow number while a count-in is set, and its tooltip says
  how long.
- During the count-in, the time display counts down the beats left (*Count-in · 3*), over the
  count-in's length (*1 bar count-in*).

::: shot count-in
crop transport.time 10
highlight transport.time "1" "The beats left before the take starts."
The time display during a count-in.
:::

The metronome's menu (the ▾ beside **Click**) sets the same count-in.

## The metronome

**Click** in the transport bar (or [[C]]) switches the metronome on and off. The ▾ beside it
holds its options:

- **Volume**, from −60 to +6 dB.
- **Only while recording**: click only during takes, not plain playback.
- **Count-in**: Off, 1 bar or 2 bars, as on the Record button.

The click is a short beep, higher on the first beat of each bar. It follows the tempo and
time signature, including tempo changes along the song. It plays on the master's outputs after
the master, so muting the master silences it too, and it is **never exported**.

::: shot main-window
crop transport.snap transport.follow 6
highlight transport.click "1" "Click: the metronome on or off."
highlight transport.click_menu "2" "Its options: volume, only while recording, count-in."
highlight transport.loop "3" "Loop."
highlight transport.punch "4" "Punch."
:::

## Punch recording

Punch records only between two points, the **punch range**, while playback runs before and
after it, so you can play along into a part and fix just that part.

1. **Shift**+drag along the ruler to draw the punch range. It shows as a strip along the
   bottom of the ruler, red while punch is on.
2. Switch **Punch** on in the transport bar (or press [[P]]).
3. Put the playhead a little before the range, arm the track and press Record.

Recording begins at the punch-in point and ends at the punch-out point, to the sample;
playback carries on. Switching Punch on with no range punches the loop range, or, with no
loop either, the four bars from the playhead's bar. With the playhead already past the end of
the range, Record says so: move it back or turn Punch off.

## Loop recording and takes

With **Loop** on, recording goes round the loop range again and again, and each pass becomes a
**take**. Play the part as many times as you like, then pick the best.

1. Drag along the ruler to set the loop range ([[page:multitrack#the-loop-range]]).
2. Switch **Loop** on (or press [[L]]).
3. Arm the track and press Record. Oxide starts at the loop's start (or plays up to it from an
   earlier playhead), and records pass after pass.
4. Stop when you have enough.

The result is one clip spanning the loop, holding a take per pass; it plays the last complete
pass. Its name ends with which take plays and how many there are, such as *· take 3/3*; each
take is named after the track and its pass (*Vocal take 3*). Loop
recording again over exactly the same range adds the new takes to the same clip. While
recording, you hear only the pass you are playing, not the earlier ones.

### Choosing a take

Right-click the clip: the menu starts with **Takes**, a line per take (*1  Vocal take 1*, …).
Click one to play it. **Delete take N** removes the take that plays now from the clip (its
file stays in `Recorded`).

To build one take from the best bits of several (comping), split the clip where the takes
should change ([[S]] at the playhead) and pick a take for each piece.

With punch on as well, each pass is recorded only inside the punch range.

## Where takes go

Every take is a WAV file in the project's `Recorded` folder, named after the track and when it
was recorded (`Vocal-1792952332.wav`). Takes are 32-bit float at the project's sample rate, mono
from a single input and stereo from a pair. A take that recorded nothing leaves no file.

Takes are never written anywhere outside the project's folder, so copying the folder takes
them along ([[page:getting-started#the-project-folder]]).

## Where takes land: latency and the record offset {#where-takes-land-latency-and-the-record-offset}

What you play reaches Oxide later than you heard the music it played, by the device's round
trip (output plus input latency), the processing block and the plug-in delay compensation.
Oxide measures the round trip from the driver and moves every take earlier by all of it, so a
take lands where you played it against what you heard. Plain takes, punch, count-in and loop
passes are all placed the same way.

If a loopback test (an output cabled to an input, playing clicks and recording them) still
shows takes off by a few samples, set the difference as **Record offset** in Settings > Audio:
positive moves takes earlier, negative later. The line under the setting shows the measured
round trip.

## Instrument tracks and MIDI

To play and record a keyboard:

1. Add an **Instrument track**. It is armed already, and listens to all MIDI inputs.
2. Add the instrument: click **+ Add plug-in…** in the inspector (or an empty slot of the
   header's rack) and choose the synth or sampler. It goes in the first insert.
3. Play: the armed track plays its instrument from the keyboard, as long as its Monitor mode
   lets you hear it (*Auto* while armed, or *Always on*).
4. Press Record. Notes, sustain and other controllers, pitch bend and program changes are
   recorded; a note still held when you stop ends there.

The take goes into a MIDI clip already there that covers the whole take, or into a new clip.
Edit it in the [[page:piano-roll]]. To start with an empty clip and an instrument in one go,
use **Insert MIDI track** ([[page:piano-roll#inserting-a-midi-track]]).

If the keyboard does nothing, check Settings > MIDI lists it
([[page:audio-device#midi-settings]]) and the track's **Input** and **MIDI channel** match it.

## Over MCP

An agent records with [[action:record]] and [[action:stop]], arms and mutes with
[[action:set_track_mixer]], sets inputs with [[action:set_track_input]] and monitoring with
[[action:set_track_monitor]]. [[action:set_punch]], [[action:set_loop]] and
[[action:set_metronome]] (which holds the count-in) set the rest; [[action:set_active_take]]
and [[action:delete_take]] pick and remove takes; [[action:get_transport]] reports the count-in
while it runs. MIDI takes can replace what they cover, or snap note starts to the grid, only
through [[action:set_midi_record_options]].

## See also

- [[page:audio-device]]: the device, the buffer size and the processing block.
- [[page:choppy-audio]]: when monitoring crackles.
- [[page:no-sound]]: when you hear nothing.
