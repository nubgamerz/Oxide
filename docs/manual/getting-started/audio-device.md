# Choosing the audio device

Oxide plays and records through one audio device at a time. You choose it in Options >
Settings… > **Audio**, together with how big a buffer the device works in and how the engine
runs on it. Nothing there changes until you press **Apply**.

::: shot settings-audio
crop settings.audio 8
highlight settings.audio.driver "1" "Driver type: WASAPI or ASIO."
highlight settings.audio.output "2" "Where the master plays (with ASIO: the ASIO driver)."
highlight settings.audio.input "3" "What tracks record from (WASAPI only)."
highlight settings.audio.sample_rate "4" "Sample rate."
highlight settings.audio.buffer_size "5" "Buffer size: the device's own buffer."
highlight settings.audio.processing_block "6" "Processing block: small buffers gathered into a bigger one."
highlight settings.audio.threads "7" "Audio threads."
highlight settings.audio.record_offset "8" "Record offset: a correction for where takes land."
highlight settings.audio.apply "9" "Apply, and what is running now."
Options > Settings… > Audio.
:::

## Pick a driver: WASAPI or ASIO

**Driver type** decides how Oxide talks to Windows' audio:

- **WASAPI** is Windows' own audio. It shares the device with other programs (a browser can
  play at the same time), works with any device, and needs no extra software. Pick an
  **Output device** (where the master plays) and an **Input device** (what tracks record
  from). *System default* follows Windows' sound settings.
- **ASIO** talks to the interface through its maker's driver, for the smallest buffers and the
  lowest delay. Instead of two devices there is one **ASIO driver** that plays and records.
  Use your interface's own ASIO driver when it has one.

Two free ASIO drivers work with devices that have none of their own:

- **FlexASIO** runs on top of WASAPI. It is set up for one device in its own configuration
  file, so it only appears in the list while that device is connected: it can't start
  otherwise. On shared WASAPI such a driver can call back in bursts (many small buffers in a
  row, then a wait), which Oxide measures as such.
- **ASIO4ALL** takes the device for itself: other programs can't play through it while Oxide
  runs. At 64 samples it is occasionally late; 128 samples avoids that.

The line next to **Apply** says what is running: *Running: device, rate, inputs / outputs*.

## Buffer size

**Buffer size** is how many samples the device hands Oxide at a time. *The driver's default*
leaves it to the driver; the list offers the sizes the device accepts, from 16 to 2048
samples.

- **Smaller** buffers mean less delay between playing a note and hearing it through your amp
  sim, but the computer must finish each buffer sooner, so heavy plug-ins can crackle.
- **Larger** buffers give every plug-in more time, at the cost of more delay.

At 48 kHz, 64 samples is 1.3 ms and 256 samples is 5.3 ms per buffer. For recording through
amp sims, start at 128 and go lower only while the [[page:choppy-audio|DSP load]] stays well
under 100 %.

## The processing block

**Processing block** gathers the device's small buffers into one larger block before Oxide
processes it. Plug-ins that cost mostly per call, such as some amp simulators, then need
far less CPU, and a driver that calls back in bursts keeps up. *Each buffer as it
comes* is off; *Gather into 64 samples* (or 32 to 512) is on.

It adds the block, less one buffer, of delay between input and output; the line under the
setting says how much with your buffer. Recordings are still placed where you heard them, and
an export runs in the same block, so it sounds exactly like playback.

[[page:choppy-audio#raise-the-processing-block]] says when to use it, with measured examples.

## Sample rate

**Sample rate** is how many samples a second the project plays at: 44100 or 48000 Hz for most
work. *Device default* uses the device's. A project made at another rate is converted when it
opens. Changing it reloads the project's plug-ins behind the loading bar.

## Audio threads

**Audio threads** is how many threads process independent tracks side by side. *Auto* uses one
per physical core but the first, which is left to the driver; *Audio thread only* processes
everything on the driver's thread. Leave it on Auto unless you are measuring. The status bar
shows the count in use.

## Record offset

**Record offset** moves recordings earlier (negative: later), in samples, on top of the round
trip Oxide measures from the driver. The line under it shows that measured round trip. Leave it
at 0 unless a loopback test (an output cabled back to an input) shows takes landing off.
[[page:recording#where-takes-land-latency-and-the-record-offset]] explains how takes are
placed.

## Apply

**Apply** restarts the audio device with the new settings, behind a short "Restarting the
audio device" overlay. A change of the processing block or the record offset alone takes
effect without a restart. Until you press it, the bottom line says *Not applied yet*.

If the device can't start, a message says why (*Audio device: …*); choose another device or
driver and Apply again. A driver whose devices can't be listed says so in red in the section.

## MIDI keyboards and controllers {#midi-settings}

::: shot settings-midi
crop settings.midi 8
highlight settings.midi.inputs "1" "The MIDI inputs Oxide opened."
highlight settings.midi.reconnect "2" "Opens every MIDI input again."
Options > Settings… > MIDI.
:::

Settings > **MIDI** lists the MIDI inputs (keyboards, controllers) Oxide opened. Every input
connected when Oxide starts is opened; there is nothing to switch on. Changes here apply at
once.

- Plugged a keyboard in after starting Oxide, or one stopped answering? Press **Reconnect MIDI
  devices** (also in the Options menu).
- Which keyboard plays which track is set per instrument track, by its **Input** and **MIDI
  channel** in the inspector: [[page:recording#instrument-tracks-and-midi]].

The status bar lists the MIDI inputs open (*MIDI: …*) or says *No MIDI inputs*.

## Over MCP

An agent reads these settings with [[action:get_audio_settings]], lists devices with
[[action:list_audio_devices]] and changes them with [[action:set_audio_settings]], as Apply
does. [[action:reconnect_midi]] reopens the MIDI inputs.

## See also

- [[page:choppy-audio]]: crackles, dropouts and a high DSP load.
- [[page:no-sound]]: nothing comes out.
