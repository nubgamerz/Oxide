# Glossary

The words this manual and Oxide use, in alphabetical order, each with where it matters.

::: shot main-window
crop status 4
highlight status.format "1" "Sample rate and buffer size."
highlight status.pdc "2" "PDC: delay compensation."
The status bar: several of these words at work.
:::

## A to C

**Arm.** Readying a track to record: its **R** button. Record records every armed track.
[[page:recording#arming-a-track]]

**ASIO.** A way for Oxide to talk to an audio interface through its maker's driver, for the
smallest buffers and lowest latency. FlexASIO and ASIO4ALL are free ASIO drivers for devices
without one. [[page:audio-device#pick-a-driver-wasapi-or-asio]]

**Automation.** Changes of a setting along the song (volume rising into a chorus), drawn as
points on a lane under the track. Each track's automation mode (Read, Touch, Latch, Write)
says whether lanes play or record what you move. [[page:mixer]]

**Bit depth.** How many bits each sample is stored with. Oxide records 32-bit float, which
can't clip inside the file; exports can be 16 or 24-bit, or 32-bit float for WAV.
[[page:exporting]]

**Buffer (buffer size).** How many samples the audio device hands Oxide at a time. Smaller is
less latency, larger gives plug-ins more time. [[page:audio-device#buffer-size]]

**Bus.** A return track fed by other tracks' sends, typically holding one shared effect such as
a reverb. [[page:multitrack#folders-buses-and-the-master]]

**Clip.** A piece of audio or MIDI on a track, playing part of a file. Editing a clip never
changes the file. [[page:multitrack#clips]]

**Count-in.** One or two bars of metronome before a recording starts.
[[page:recording#the-count-in]]

**Crossfade.** Two overlapping clips, one fading out as the other fades in, so the join is
seamless. [[page:multitrack#fades-and-clip-gain]]

## D to L

**Delay compensation (PDC).** Some plug-ins hold the audio back to do their work (a look-ahead
limiter, a linear-phase EQ). Oxide delays every other path by the same amount so everything
stays in time. The status bar's *PDC* is the total; it adds to the latency you hear while
monitoring. [[page:recording#latency-while-monitoring]]

**Dither.** A tiny, deliberate noise added when an export is written at 16 or 24 bits, so quiet
fades don't turn into distortion. Oxide adds it to integer formats unless you turn it off.
[[page:exporting]]

**DSP load.** How long Oxide takes to process one buffer, as a share of the time the device
allows for it. Over 100 % is a dropout. **Peak** is the worst buffer of the last two seconds.
[[page:choppy-audio#the-dsp-readout]]

**Folder.** A track that holds other tracks and mixes them, like a submix, with its own fader
and plug-ins. [[page:multitrack#folders-buses-and-the-master]]

**Grid.** The lines along the timeline that the playhead, clips and notes snap to: a fixed
division (1/4 to 1/64, or triplets) or Auto, which follows the zoom.
[[page:multitrack#the-grid-and-snap]]

**Input gaps.** Buffers where the input device hadn't delivered audio in time; usually two
devices running on different clocks. [[page:choppy-audio#input-gaps-and-skips]]

**Insert.** A plug-in in a track's chain: the audio runs through each insert in turn. An
instrument track's first insert is its instrument. [[page:multitrack#the-inspector]]

**Latency.** The delay between something happening and hearing it: from playing a note to
hearing it through your amp sim, the buffer, the device's round trip and the delay
compensation add up. [[page:recording#latency-while-monitoring]]

**Loop range.** The span of the song that plays round and round while Loop is on, and that loop
recording records a take per pass of. [[page:multitrack#the-loop-range]]

**LUFS.** Loudness Units relative to Full Scale: loudness as the ear hears it, measured over a
whole song (integrated, *I* in the transport bar). Streaming services aim at about −14 LUFS.
[[page:exporting]]

## M to R

**Marker.** A named place in the song; keys 1 to 9 go to the first nine.
[[page:multitrack#markers]]

**Master.** The mix of every track, and what plays out of the speakers and into an export.
[[page:multitrack#folders-buses-and-the-master]]

**Monitoring.** Hearing your input through a track (and its plug-ins) as you play.
[[page:recording#monitoring-while-playing-and-recording]]

**Plug-in skips.** Buffers a plug-in was left out of because something else held it for a
moment (saving its state); the track is silent for that buffer. Not the CPU running out.
[[page:choppy-audio#input-gaps-and-skips]]

**Processing block.** Oxide gathering the device's small buffers into one larger block before
processing it, so plug-ins that cost per call use far less CPU, for a little more latency.
[[page:choppy-audio#raise-the-processing-block]]

**Punch.** Recording only between two points while the song plays before and after.
[[page:recording#punch-recording]]

**Quantize.** Moving notes or audio slices onto the grid. [[page:piano-roll#quantize]],
[[page:multitrack#slicing-at-transients-and-quantizing-audio]]

**Render = preview.** Oxide's promise that an export sounds exactly like playback: it runs
through the same engine, plug-ins and block size. [[page:index]]

**Round trip.** The time from Oxide sending audio out to the same moment coming back in through
an input: the device's output and input latency. Oxide measures it and places takes by it.
[[page:recording#where-takes-land-latency-and-the-record-offset]]

## S to Z

**Sample rate.** How many samples make a second of audio: 44100 or 48000 for most work.
[[page:audio-device#sample-rate]]

**Send.** A copy of a track's sound sent to a bus, at its own level, before (pre) or after
(post) the track's fader. [[page:multitrack#folders-buses-and-the-master]]

**Snap.** Whether the playhead, clips, selections and markers jump to the grid. **Alt** turns it
off for one drag in the piano roll; **Ctrl** turns off the playhead magnet.
[[page:multitrack#the-grid-and-snap]]

**Solo.** Playing only the soloed tracks (and what they feed). [[page:no-sound#one-track-is-silent]]

**Take.** One recording of a part. Loop recording makes a take per pass, all in one clip, and
you choose which plays. [[page:recording#loop-recording-and-takes]]

**Time selection.** A span of time across tracks, drawn by dragging in empty lanes; split,
delete, loop or export it. [[page:multitrack#time-selections]]

**Transient.** The sharp start of a note or hit. Slicing cuts a clip at its transients; Keep
attacks keeps them sharp when stretching. [[page:multitrack#trimming-and-stretching]]

**True peak.** The highest level the audio reaches between samples once it is turned back into
sound, a little higher than the highest sample. *TP* in the transport bar; keep it under −1 dBTP
for streaming. [[page:exporting]]

**Velocity.** How hard a MIDI note is played, 1 to 127. [[page:piano-roll#velocity]]

**VST3.** The plug-in format Oxide hosts. [[page:plugin-problems]]

**WASAPI.** Windows' own audio, shared with other programs.
[[page:audio-device#pick-a-driver-wasapi-or-asio]]
