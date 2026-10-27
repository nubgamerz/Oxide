# dawpoc

First proof of concept for a custom Rust DAW engine. It hosts real VST3 **effect** plugins
in-process (via `vst3-host` 0.10) and measures the two problems the DAW exists to fix:
**render differs from preview**, and **poor multithreading**.

There are no instruments, MIDI, plugin GUIs or live audio output yet.

## Design

- `src/engine.rs`: `Chain` is an ordered list of plugins with one `process_block` function.
  Offline render calls it today, and the live callback will call the same function, so
  preview and bounce can only differ through the VST3 `ProcessMode`, which is explicit.
  Per plugin it activates the main input and output buses, applies the process mode, starts
  processing, services restart requests (to pick up latency the plugin reports on activation),
  sets the transport to playing, and preallocates bus buffers. `process_block` is
  allocation-free and uses the bus-aware `process_bus_audio`, so sidechain buses keep their
  indices and stay silent. Channel counts are adapted between stages (mono to stereo
  duplicates; many to mono averages).
- `render_aligned` feeds fixed-size blocks, drops the chain's reported latency from the start
  and flushes it with silent blocks, so `out[n]` lines up with `in[n]`.
- `src/null.rs`: null metrics, a verdict, a per-segment residual and a cross-correlation lag search.
- `src/bench.rs`: N independent instances, processed serially and then with `std::thread::scope`.
  Each instance is only touched by one thread at a time. `Plugin` is `Send` as published, so
  this needed no `unsafe`.

## Usage

```sh
cargo build --release -p dawpoc
B=target/release/dawpoc

$B list [filter]                              # installed .vst3 bundles (nothing is loaded)
$B gen --output out/test.wav                  # 10 s, 48 kHz, stereo f32, seed 1
$B info --plugin "<plugin>"                  # buses, latency/tail in both modes, parameters
$B render --plugin "<plugin>" --input out/test.wav --output out/a.wav \
          --mode realtime --block 512 [--preset x.vstpreset] [--set "C1 B1 Threshold=0.6"] \
          [--preroll 2] [--keep-tail] [--skip-failed]
$B null --a out/a.wav --b out/b.wav [--search-lag 1024] [--segment 0.5]
$B bench --plugin "<plugin>" --instances 8 --seconds 10 [--threads 8] [--block 512] [--set ...]
```

- `--plugin` accepts a path or a case-insensitive installed name. Repeat it to build a chain.
- `--set NAME=VALUE` sets a normalized (0..1) parameter. Use `info --all-params` to see the
  names and formatted values.
- `--preset` applies by position to the plugin at the same index. It accepts `.vstpreset`, or
  the JSON written by `vst3-host`'s `save_preset`.
- `--preroll S` processes S seconds of silence, discarded, before the render. See the amp sim A
  finding below.
- The test signal has 0.1 s of silence, then pink-noise bursts rising from -30 to -4 dB, then
  four chord hits peaking at -12, -6, -3 and -1 dBFS, then a 20 Hz to 20 kHz log sweep, then
  silence.

The null verdict is `bit-identical`, `below -120 dBFS`, `AUDIBLE` (residual peak above
-120 dBFS) or `INVALID` (NaN or infinity). Note that "AUDIBLE" only means "above -120 dBFS".
A -90 dB residual will not actually be heard.

## Findings (Ryzen 7 7700X 8C/16T, Windows 11, 48 kHz, 2026-10-18)

Every plugin was rendered in its own process. Unless stated otherwise, each one ran with the
default state it loads in.

### Latency reporting
| plugin | at load (vst3-host's 44.1 kHz default) | after activation at 48 kHz | restart flag on activation |
|---|---|---|---|
| Channel-strip compressor | 43 | 0 | kLatencyChanged |
| Mastering maximizer | 747 | 810 (1596 at 96 kHz) | kLatencyChanged |
| Mastering limiter | 721 | 784 | kLatencyChanged |
| Amp sim A / B | 55 / 98 | 55 / 98 | A: kParamValuesChanged only |

There are two separate traps here.

1. `vst3-host` runs the first `setupProcessing` inside `load_plugin` at its 44.1 kHz default,
   before it applies the host's sample rate. Any latency read straight after loading is
   therefore for the wrong rate.
2. When the plugin is activated at the real rate, the mastering and channel-strip plugins raise
   `restartComponent(kLatencyChanged)`. The new value only appears once the host services
   that request, which `Stage::start` does. At 44.1 kHz the Maximizer raises no flag and
   stays at 747.

The channel-strip compressor reports 43 before activation and 0 after it, even at 44.1 kHz. That is
unexplained, but the measured lag of 0 confirms that 0 is correct.

The value read after activation is right for every plugin tested:
- The default mastering maximizer nulls **bit-identically** against the input once 810 samples
  are trimmed.
- The lag search finds 0 for every mastering and channel-strip plugin.

With the load-time value, the renders would have been misaligned by 43 samples (channel-strip
compressor) or 63 samples (mastering suite), which puts a comb filter into any parallel mix.

### a. Realtime vs offline (block 512)
| plugin | result |
|---|---|
| Channel-strip compressor (default, and thresholds at 0.6) | bit-identical |
| Channel-strip equalizer (default, and two bands enabled with gain) | bit-identical |
| Mastering maximizer (default, and Input Gain 0.5 so it limits) | bit-identical |
| Mastering limiter (default, -2 dB threshold, engages) | bit-identical |
| Amp sim B | bit-identical |
| Reverb | bit-identical |
| Amp sim A, cold start | residual peak -11.8 dBFS, rms -36.7 dBFS, all of it in the first ~1 s |
| Amp sim A, with `--preroll 2` | residual peak -138.5 dBFS (not bit-identical, but inaudible) |

### b. Block size (realtime, 64 and 4096 vs 512)
- The mastering and channel-strip plugins (all of the above), the reverb: bit-identical.
- Amp sim B: about -133.6 dBFS peak (float rounding).
- Amp sim A with pre-roll: 64 vs 512 gives a -53.3 dBFS peak and -86.8 dBFS rms. 4096 vs 512 gives a
  -54.9 dBFS peak and -90.0 dBFS rms. The difference spans the whole file, so the output
  genuinely depends on block size.

### c. Run-to-run determinism (same settings, twice)
- Everything is bit-identical except **amp sim A**. Two cold runs differ by up to
  -12 to -20 dBFS peak in the first ~1 s, and after that by about -144 dBFS.
- A wall-clock wait after activation (`--settle-ms 3000`) does **not** help. Processing 2 s of
  silence first (`--preroll 2`) makes amp sim A bit-identical run to run.
- The cause looks like internal DSP state that is not reset on activation, and that decays
  while processing. This is an inference; it was not verified inside the plugin.
- This is a real render/preview mismatch mechanism. Live playback has been processing for a
  while before you hit play, but a bounce started right after load starts cold.
- A mode switch re-activates the plugin, so it is *expected* to cause the same cold start.
  That was not measured.
- Either way, the engine should always pre-roll after activation.

### d. Serial vs parallel (block 512, realtime, 1 thread per instance unless noted)
| plugin | N | serial wall | parallel wall | speedup | aggregate RT factor, parallel |
|---|---|---|---|---|---|
| Amp sim A, 10 s | 4 | 3.48 s | 0.92 s | 3.79x | 43.6x |
| Amp sim A, 10 s | 8 | 6.63 s | 0.97 s | 6.81x | 82.2x |
| Amp sim A, 10 s | 16 | 11.90 s | 1.16 s | 10.28x | 138.3x |
| Amp sim A, 10 s, 8 threads | 16 | 11.93 s | 1.93 s | 6.20x | 83.2x |
| Amp sim A offline, 10 s | 8 | 7.02 s | 0.98 s | 7.18x | 81.8x |
| Channel-strip compressor (engaged), 120 s | 4 | 0.64 s | 0.16 s | 3.96x | 2966x |
| Channel-strip compressor (engaged), 120 s | 8 | 1.27 s | 0.19 s | 6.73x | 5075x |
| Mastering maximizer (engaged), 30 s | 4 | 0.56 s | 0.15 s | 3.78x | 805x |
| Mastering maximizer (engaged), 30 s | 8 | 1.11 s | 0.17 s | 6.70x | 1446x |

- A repeat of amp sim A at 4 and 8 instances gave 3.82x and 6.66x.
- Independent instances scale almost linearly up to the 8 physical cores. 16 threads on
  8C/16T reached 10.3x.
- One amp sim A instance costs about 0.8 ms per 512-sample block (10.67 ms deadline).
- With 16 busy threads, the worst amp sim A block hit 11.08 ms, which is over the deadline.
  Oversubscribing the SMT siblings hurts worst-case latency even when throughput rises.
- So these plugins are not inherently hard to parallelize. Poor multithreading in the current
  DAW is more likely a scheduling problem in the host. This is an inference.

### e. Cost per call: amp sims at tiny buffers (2026-10-24, dev plan 23.1)

A maintainer's offline check (kept outside the repository) times one instance, set up as the
session does (48 kHz, `MAX_BLOCK` 2048, activated and pre-rolled), a guitar recording fed in
calls of each size, every call timed. "RT load" is the mean call over the call's own audio
(100%: a whole core). "Heavy" is a saved state with more of the amp's stages switched on.

| plug-in | 16 | 32 | 64 | 128 | 256 | 480 | fit: per call + per sample |
|---|---|---|---|---|---|---|---|
| Amp sim B (default) | 347 µs, 104% | 391, 59% | 471, 35% | 634, 24% | 1001, 19% | 1589, 16% | 302 µs + 2.7 ns (88% of a 16-sample call fixed) |
| Amp sim C (default) | 179 µs, 54% | 249, 37% | 388, 29% | 680, 26% | 1271, 24% | 2299, 23% | 100 µs + 4.6 ns (58%) |
| Amp sim C (heavy) | 334 µs, 100% | 457, 69% | 715, 54% | 1208, 45% | 2248, 42% | 3817, 38% | 235 µs + 7.5 ns (66%) |
| Amp sim D (heavy) | 625 µs, 188% | 650, 98% | 699, 52% | 796, 30% | 1057, 20% | 1451, 15% | 587 µs + 1.8 ns (95%) |
| Amp sim A (default) | 46 µs, 14% | 76, 11% | 131, 10% | 244, 9% | 488, 9% | 899, 9% | 14 µs + 1.8 ns (33%) |

- These amp sims (A aside) pay a large fixed cost every call, whatever its length: at a
  16-sample call one track of B or a heavy C needs a whole core, a heavy D
  nearly two. No host can play them at 16-sample calls on this CPU. None of them works on
  threads of its own (the process's CPU time equals the calling thread's).
- Oxide's own work per callback at 16 samples is about 5 µs (one track of Amplify on FlexASIO:
  1.5% of a burst; `DAW_TRACE=1`: 0-1 µs before the graph, 1-2 µs after it), and a level of one
  node never wakes the worker pool. Nothing in the engine's path is worth cutting here.

**Live, FlexASIO on a USB guitar interface** (a `FlexASIO.toml` set to shared WASAPI, 16-sample buffers,
bursts of 480 every 10 ms), master muted, one track playing a guitar recording (a maintainer's
check, kept outside the repository). The processing block gathers the device's buffers
into one block before processing (Settings > Audio; it adds the block less one buffer of
latency). DSP mean (against the burst's real deadline) and the CPU panel's figure for the track:

| plug-in | 16 (off) | 32 (+16) | 64 (+48) | 128 (+112) |
|---|---|---|---|---|
| Amp sim B (default) | 106% / 105%, every interval over | 61% / 60% | 39% / 37% | 27% / 25% |
| Amp sim C (default) | 56% / 55% | 40% / 39% | 33% / 31% | 29% / 27% |
| Amp sim C (heavy) | 102% / 103%, every interval over | | 57% / 55% | 50% / 48% |
| Amp sim D (heavy) | 191% / 191%, every interval over | | 55% / 54% | 35% / 33% |
| Amp sim D (default) | 102% / 102%, every interval over | | 34% / 32% | |
| Amp sim A (default) | 16% / 14% | | 12% / 10% | |

- The DSP figure is right: it matches the CPU panel within a few points, and with the plug-in
  over 100% the driver is kept behind (callbacks a period apart, no waits), a true overload.
- One minute at 64 (+1.0 ms): amp sim B mean 38%, worst interval 80%; C 33%, worst 68%;
  heavy C 57%, worst 89%; heavy D 55%, worst 77%. 0 of 599 intervals over the
  deadline and 0 missed blocks in each.
- Gathering can't be free of latency: ASIO hands the host 16 input frames per callback, so a
  block can run only once its last buffer is in, and plays out after it.

### Not verified / caveats
- Whether the mastering and channel-strip plugins actually change anything internally in `kOffline`: they produce
  identical output, which is the result that matters here, but nothing more can be seen.
- The mastering and channel-strip plugins were only tested with default settings and a few
  `--set` tweaks, not with real-world presets. Rerun with `--preset` to cover those.
- Whether any of these plugins were running in a trial or unauthorized mode. They all load
  and process audibly, and no plugin failed to load.
- One reverb plugin reports a tail of 0 even though it has one, so `--keep-tail` cannot
  rely on it. A synth's bundled effects plugin reports 96000.
- Process isolation (`--isolated`) is not implemented. `vst3-host`'s helper binary is a bin
  target of the dependency and cargo does not build it for us.
- `vst3-host` locks per-instance mutexes inside `process_bus_audio` (metering, and the
  editor's parameter queue). They are uncontended here, but they must go before a live
  callback exists.
