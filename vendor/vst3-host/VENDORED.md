# Vendored: vst3-host 0.10.0

The crates.io crate `vst3-host` 0.10.0 (Helge Sverre, MIT). The workspace uses it through
`[patch.crates-io]` in the root `Cargo.toml`.

## Why

The crate keeps a plug-in behind one `Mutex<Plugin>`, and `PluginWindow::open` held that lock
while the plug-in created and attached its editor view (and before that, while a throwaway view
was made just to read the editor's size). The DAW's audio thread only `try_lock`s a plug-in, so
for that time the track went silent: Archetype Nolly's editor held it for about 250 ms (TD3).
VST3 itself lets the edit controller run on the UI thread while the processor runs on the audio
thread; the lock was the crate's choice, not the spec's.

## Changes from the crate

- `src/internal/plugin_impl.rs`: `EditorSetup` (the controller and host frame, taken under the
  lock), `EditorView` (created and sized without it) and `AttachedEditor` (attached without it,
  handed back under it; detached without it). `PluginImpl::open_editor` and `close_editor` now
  run those same steps, so the old entry points behave as before. New trait methods
  `begin_editor`, `adopt_editor` and `release_editor`.
- `src/plugin.rs`: `Plugin::begin_editor`, `adopt_editor` and `release_editor`; the trait's
  defaults refuse (process isolation keeps the old path). The three types are re-exported.
- `src/window.rs` (Windows only): `PluginWindow::open` uses the split path (no throwaway size
  view: the window takes the real view's size) and falls back to the old one when refused;
  `close` takes the view out under the lock and detaches it outside. macOS and Linux are
  unchanged and untested.
- Left out: the crate's `Cargo.lock` and `CHANGELOG.md`.

Measured (`crates/daw-core/examples/editor_check.rs`, Archetype Nolly): the longest time the
audio thread found the lock taken went from 254 ms to 0 ms; opening the editor got faster
(about 540 ms to about 300 ms), since no throwaway view is made.

A fix to the first change: an editor that gives no size until it is attached (Kontakt 8
answers `getSize` with an error before) opened with "Failed to get view size". `EditorSetup::
create` now starts such a view at 800x600, as the crate's own `get_editor_size` does, and
`PluginWindow::open` fits the window to `AttachedEditor::size` once the view is attached.

A third change, so editors follow their windows: dragging an editor window's edge resized
only the window (black space beside a plug-in that never heard of it). `src/window.rs`: the
window procedure records `WM_SIZE` (and the drag's start and end); `service_platform_events`
hands the size to the view through a `ViewHandle` (a second COM reference to the attached
view, from `AttachedEditor::handle`, so no plug-in lock is taken; dropped before the view is
detached) via `resize_plug_view` (`canResize`, `checkSizeConstraint`, `onSize`; the old
`resize_editor` now calls it too), and snaps the window to the size the view took once the
drag ends. A view that can't resize gets a window without a resize border, and
`resize_native_window` sizes by the window's actual style. `src/internal/com_implementations.rs`:
`HostPlugFrame` has a slot for the editor's window (`window_slot`, filled by `PluginWindow`
while the view is attached) and `resizeView` resizes that window before calling `onSize`, as
the SDK's host does (Kontakt's own corner grip widened nothing when the window followed a frame
later). `resize_plug_view` returns the size the view reports after `onSize`. For checks:
`drag_to`, `client_size` and `pump_messages`.

A second change, for multi-out instruments (dev plan 14.4):

- `src/plugin.rs` and `src/internal/plugin_impl.rs`: `Plugin::audio_output_names`, the audio
  output buses' names read from the loaded component (`getBusInfo`), so a host can name the
  tracks of a multi-out rack ("KT Out 2") without loading a second instance through
  `get_detailed_plugin_info`. The internal trait's default refuses, so process isolation is
  unchanged.

Everything else is byte for byte the crate as published.

A fourth change, so parameters never take the plug-in's lock (dev plan 18.4, TD22, TD61): an
agent reading or setting parameters during playback, and the editor-gesture poll while writing
automation, held the lock the audio thread `try_lock`s, and each time they met, the track's
block went silent (`param_check`: 32-65 blocks on Ozone 11 Maximizer and 30-61 on Neutron 5
Compressor in four seconds of an agent's loop at 128 samples, 2-8 from the gesture poll with
the editor open in Touch and no knob moved). VST3 lets the edit controller run on the UI thread
while the processor runs, so:

- `src/internal/plugin_impl.rs`: `ControllerLink`, taken once under the lock
  (`PluginInternal::controller_link`, `Plugin::controller_link`; the trait's default is `None`,
  so process isolation is unchanged): a second reference to the edit controller and to the
  component handler, and the host's queues around them. On the control thread only, without
  the lock, it reads every parameter (`parameters`, the same code `get_all_parameters` now
  shares: `read_parameters`), a value (`value`), the plug-in's display text
  (`format`, shared with `format_parameter`: `format_parameter_with`), sets a value (`set`:
  `setParamNormalized` now, and a new lock-free inbox for the processor), drains the editor's
  gesture log (`take_edits`), applies values queued from the audio thread to the controller and
  reports the processor's output values (`take_values`), peeks at the `restartComponent` flags
  (`pending_restart`) and takes `kParamValuesChanged` alone (`take_values_changed`; the locked
  servicing does nothing with that flag). `report_edits_as_editor` (hidden) drives the component
  handler's `beginEdit`/`performEdit`/`endEdit` for checks. Drop the link before the plug-in: it
  holds the controller, whose code goes with the module.
- `PluginImpl::host_inbox` (an `ArrayQueue`, 1024 values; a set past that is refused, not
  dropped): drained in `process_chunk` at the start of the block (offset 0, before the block's
  own queued changes, so a later automation point still lands after it), on a zero-sample flush
  too (saving state flushes first), and emptied when a state is restored, as the pending changes
  are. `deferred_controller_sync` is now an `Arc` so the link can drain it.
- `src/internal/com_implementations.rs`: `ComponentHandler::peek_restart_flags` and
  `take_restart_bits`.
- `src/lib.rs` re-exports `ControllerLink`.

Measured after (`param_check`, the same runs): zero blocks missed on both tracks; a value set
through the link before block 20 plays bit for bit as one set under the lock, and changes the
output from that block's first sample.

A fifth change, so an open editor never takes the plug-in's lock (TD148): with an editor open,
`PluginWindow::service_platform_events` (called every frame) `try_lock`ed the plug-in to read the
editor's resize request and apply a pending DPI change, holding it well under a microsecond each
time. About once in a few hundred frames that fell on the audio thread's own `try_lock`, and the
track's block went silent: `param_check` missed one Ozone 11 Maximizer block in its editor phases
in about half the runs (traced: every miss fell while this call held the lock, about 320 takes
in each four-second phase; nothing else took it). Neither needs the processor:

- `src/internal/plugin_impl.rs`: `EditorSetup::resize_requests`, the host frame's resize slot
  (the `editor_resize` that `take_editor_resize_request` reads); `ViewHandle::set_scale_factor`,
  the content scale offered through the window's own reference to the view.
- `src/window.rs`: `PluginWindow` keeps the slot (`resize_requests`, from `open_unlocked`, dropped
  in `close`), and `service_platform_events` services an editor opened that way from the slot and
  the `ViewHandle`, without the lock. The old locked path stays for an editor opened the old way
  (process isolation, or a plug-in that refused `begin_editor`); a DPI change no longer updates
  the plug-in's remembered scale, which only an editor opened the old way would use (the split
  path attaches at its window's DPI).

Measured after (`param_check`): no plug-in lock taken on the main thread in either editor phase,
and ten runs in a row with zero blocks missed.

## Wording (2026-10-27, dev plan 28.5)

Three doc comments (`src/audio.rs`, `src/playback.rs`, `src/simple.rs`) say "previewing" for
playing effects to hear them, so the repository's word guard (`crates/manual/tests/banned_words.rs`)
passes; no code changed.
