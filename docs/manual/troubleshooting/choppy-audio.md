# Crackling, choppy audio and high DSP

Crackling, clicks, stutters or short gaps while playing or recording mean the computer did not
finish an audio buffer before the audio driver needed it. Oxide shows when that happens and
where the time goes; this page explains the readouts, then what to change to stop it, cheapest
first.

::: shot main-window
crop transport.inspector transport.dsp 8
highlight transport.dsp "1" "The DSP readout: the load, and the worst buffer of the last two seconds. Click it for the CPU panel."
The right end of the transport bar, along the top of the window.
:::

## What the readouts mean

### The DSP readout

At the right end of the transport bar, `DSP 35% · peak 62%` and a small bar:

- **DSP** is the time spent processing each audio buffer, as a share of the time the driver
  allows for it (the buffer's duration). The bar is green, turns yellow above 60 % and red
  above 90 %.
- **peak** is the worst buffer of the last two seconds, drawn as the white line on the bar.
  Its text turns red over 100 %: that buffer was late, and you heard a dropout.

There is load while the transport is stopped: every track and plug-in keeps running on
silence, so effects' tails ring out and input monitoring works.

With a driver that calls back in bursts (some drivers, such as FlexASIO on shared WASAPI, ask
for many small buffers back to back, then wait), each buffer is measured against the whole burst's time, since that
is its real deadline. Settings > Audio shows such a driver's cadence ("in bursts of 480").

### The status bar

::: shot main-window
crop status.device status.pdc 6
highlight status.device "1" "The driver and the devices playing and recording."
highlight status.format "2" "The sample rate and the device's buffer (with the processing block, when it gathers)."
highlight status.pdc "3" "The delay compensation (PDC), the latency your plug-ins add: not a load."
The status bar, along the bottom of the window.
:::

### Input gaps and skips

The status bar shows these only when they are not zero:

- **input gaps** counts blocks where the input device had not delivered audio in time. A few
  at start-up are normal. A count that keeps rising means the input and output run on
  different clocks, usually because they are two different devices: use one interface for
  both.
- **plugin skips** counts blocks a plug-in was left out because something else held it for a
  moment (saving its state); its track is silent for that block. **engine skips** counts
  blocks played as silence while an export ran. Neither is the CPU running out.

## Find what is heavy: the CPU panel

Click the DSP readout to open the **CPU use** window. It lists every track, and every plug-in
on it, with its time per buffer and its share of the deadline, most costly first.

- Tracks run side by side on the audio threads, so their shares can add up past 100 % without
  a dropout; the DSP readout is the one that counts for the whole buffer.
- **Whole app** is all of Oxide's CPU time on every thread, including what plug-ins do on
  threads of their own (sample streaming, analysis), which the rows below can't see.
- A bypassed plug-in is not called at all, so it costs nothing (its delay compensation still
  runs). Bypassing the heaviest one is the quickest way to hear whether it is the cause.

An AI agent reads the same numbers with [[action:get_cpu_usage]], and the overall load with
[[action:get_stats]].

## Raise the processing block

Some plug-ins cost mostly **per call**, whatever the length of the buffer they are given:
some amp-simulator plug-ins take a fixed fraction of a millisecond a call, however short the
buffer. At very small buffers (16 samples, say) one such track can need a whole core, and the DSP
readout sits over 100 % however fast the computer is.

The processing block gathers the device's small buffers into one larger block before Oxide
processes it, so those plug-ins are called far less often:

1. Open Options > Settings… and choose **Audio** on the left.
2. Set **Processing block** to *Gather into 64 samples*. If the peak still goes over 100 %,
   try 128.
3. Press **Apply**. The new block takes effect at once, without restarting the device.
4. Play the part that was choppy and watch the peak.

::: shot settings-audio
crop settings.audio 8
highlight settings.audio.buffer_size "1" "Buffer size: the device's own buffer."
highlight settings.audio.processing_block "2" "Processing block: buffers gathered into a bigger block."
highlight settings.audio.apply "3" "Apply restarts the audio with the new settings."
dim
Options > Settings… > Audio.
:::

What it costs: the block, less one buffer, of extra delay between input and output; the line
under the setting says how much for your buffer size (at 48 kHz, gathering 16-sample buffers
into 64 adds 48 samples, 1 ms). Recordings are still placed where you heard them, and an
export runs in the same block, so it sounds exactly like playback. *Each buffer as it comes*
turns it off.

A measured example: one track with a heavy amp-sim plug-in at 16-sample buffers ran at 106 to
191 % (dropouts all the time); with a processing block of 64 it played at 39 to 57 % with
nothing late for a minute. Your numbers depend on the plug-in and the computer.

## Or raise the buffer size

**Buffer size**, in the same section, sets the device's own buffer. Smaller buffers mean less
delay when you monitor through plug-ins, but more work for the CPU; larger ones give every
plug-in more time. Some drivers (ASIO4ALL, for one) are occasionally late at 64 samples, where 128 avoids it. If
monitoring delay matters more than load, prefer the processing block: it adds only what it
gathers.

## Audio threads

**Audio threads** (Settings > Audio) sets how many threads process independent tracks side by
side. *Auto* uses one per physical core but the first, which is left to the driver; *Audio
thread only* turns parallel processing off. Leave it on Auto unless you are measuring.

## Over MCP

An agent sets the same options with [[action:set_audio_settings]]: `processing_block` in
samples (0 is off), `buffer_size` (0 for the driver's default) and `threads`. Like Apply, it
restarts the audio device when the buffer size or threads change; the processing block alone
needs no restart.

## See also

- [[page:#find-what-is-heavy-the-cpu-panel|The CPU panel]], above, to find the plug-in that
  costs the most.
- [[page:index#searching|Searching this manual]].
