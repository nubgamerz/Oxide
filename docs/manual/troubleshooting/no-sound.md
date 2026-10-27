# No sound

Nothing comes out, or one track is silent, or you can't hear your guitar. Work down the list:
the causes are in the order they are most often met.

::: shot main-window
crop master.mute track.header 6
highlight master.mute "1" "The master's mute: lit, nothing plays."
highlight track.mute "2" "M: this track muted."
highlight track.solo "3" "S: soloed tracks silence the others."
highlight track.power "4" "The power button: off, the track is silent."
highlight track.monitor "5" "Input monitoring."
The master's row and a track's header.
:::

## Nothing plays at all

1. **Is a device running?** The status bar's left end names the driver and devices. In
   Options > Settings… > **Audio**, the line beside Apply should say *Running: …*; *No device
   running* means none started. Choose an **Output device** (or ASIO driver) and press
   **Apply** ([[page:audio-device]]).
2. **Is it the right output?** *System default* follows Windows' default device: headphones
   plugged into the interface won't hear a laptop's speakers. Pick the interface by name.
3. **Is another program holding the device?** ASIO drivers (ASIO4ALL in particular) and
   programs using WASAPI exclusive mode take a device for themselves. Close the other program,
   or choose WASAPI in Oxide, and Apply again.
4. **Is the master muted?** Its **M**, at the top of the track list and on the mixer's master
   strip, is lit when muted. Muting the master also silences the metronome and the Waveform
   view's playback.
5. **Is the master fader down?** Select the master (**Master** beside + Track) and check its
   **Volume** in the inspector.

## One track is silent

- **Muted (M)?** Click M again.
- **Another track soloed (S)?** While any track is soloed, only soloed tracks (and what they
  feed) play. Click the lit S to clear it.
- **Switched off?** The power button at the right of its header is green when on. An off track
  (also Track > Disable track) makes no sound.
- **Volume at -inf, or a clip's gain at the bottom?** Click the volume number and type 0, or
  double-click a clip's gain line for 0 dB.
- **Where does it go?** Its **Output** in the inspector should be its folder or the master. A
  pair of the device's outputs (*Out 3+4 (direct)*) skips the master.
- **An instrument track with no instrument?** It needs an instrument plug-in in its first
  insert ([[page:piano-roll#instrument-tracks-and-their-plug-ins]]), and one that loaded: a
  name struck through failed to load ([[page:plugin-problems]]).

## You can't hear your input

To hear a guitar or a microphone through a track while you play:

1. The track's **Input** (inspector, *Routing*) is the input it is plugged into.
2. The device has inputs: with WASAPI, an **Input device** is chosen in Settings > Audio; the
   status bar shows the inputs count (*2 in*).
3. The track is **armed** (R) with **Monitor** on *Auto*, or Monitor is *Always on*. *Off*
   never plays the input.
4. The track is not muted, and its plug-ins are switched on.

[[page:recording#monitoring-while-playing-and-recording]] explains monitoring in full.

For a MIDI keyboard, check that Settings > MIDI lists it, that the instrument track's **Input**
and **MIDI channel** match it, and that the track is armed (or Monitor is *Always on*).

## The metronome is silent

Check **Click** is lit, its ▾ **Volume** isn't at −60 dB, and **Only while recording** isn't
ticked while you only play. The click plays on the master's outputs, so a muted master
silences it.

## Over MCP

[[action:get_meters]] shows which tracks have signal, [[action:get_audio_settings]] the
device, and [[action:get_project]] each track's mute, solo and output.

## See also

- [[page:choppy-audio]]: sound that crackles or drops out.
- [[page:audio-device]]: choosing the device.
