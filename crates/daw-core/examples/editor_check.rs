//! Opening a plug-in's editor must not silence its track (TD3). The audio thread only
//! `try_lock`s a plug-in, so for as long as opening the editor holds the plug-in's lock, the
//! track's blocks are skipped.
//!
//! An "audio" thread here does what the engine does: every millisecond it `try_lock`s the
//! plug-in and processes a 480-sample block. Meanwhile the main thread opens the editor (a
//! real window, closed again after half a second). Measured:
//! - **the longest lock hold**: the longest run of failed `try_lock`s. It must stay under one
//!   block (10 ms at 480 / 48 kHz), so no block is skipped;
//! - **the longest loop gap**: the longest time between two turns of the audio thread's loop,
//!   whatever the lock did (a control: if it is long, the machine stalled, not the lock).
//!
//! It also drags the window bigger (`PluginWindow::drag_to`): the editor takes that size as
//! far as it allows (a fixed-size one keeps its own), the window must end up fitting it
//! exactly (no black space), and none of it may hold the lock.
//!
//! Opens a plug-in window on the desktop for about a second. Prints `RESULT: PASS` or `FAIL:`.
//!
//! cargo run --release -p daw-core --example editor_check -- ["<plugin>"]

use anyhow::{anyhow, bail, Result};
use daw_core::plugins;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use vst3_host::PluginWindow;

const BLOCK: usize = 480;
const RATE: f64 = 48_000.0;

fn main() -> Result<()> {
    let name = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("name the plug-in as the argument"))?;
    let mut host = plugins::host(RATE, BLOCK)?;
    let installed = plugins::installed(&host);
    let path = plugins::resolve(&name, &installed)?;
    let mut plugin = plugins::load(&mut host, &path, None)?;
    plugins::activate(&mut plugin, BLOCK, BLOCK * 10, None)?;
    let mut buffers = plugin
        .create_bus_audio_buffers(BLOCK)
        .map_err(|e| anyhow!("{name}: buffers: {e}"))?;
    let plugin = Arc::new(Mutex::new(plugin));

    // The audio thread: try the lock every millisecond, process a block when it gets it.
    let running = Arc::new(AtomicBool::new(true));
    let audio = {
        let (plugin, running) = (plugin.clone(), running.clone());
        std::thread::spawn(move || {
            // Every hold (when it began, how long) and every slow turn, to place them.
            let (mut holds, mut gaps) = (Vec::new(), Vec::new());
            let (mut blocks, mut skipped) = (0u64, 0u64);
            let mut held_since: Option<Instant> = None;
            let mut last_turn = Instant::now();
            while running.load(Ordering::Relaxed) {
                let now = Instant::now();
                gaps.push((last_turn, now - last_turn));
                last_turn = now;
                match plugin.try_lock() {
                    Ok(mut p) => {
                        if let Some(since) = held_since.take() {
                            holds.push((since, now - since));
                        }
                        let _ = p.process_bus_audio(&mut buffers);
                        blocks += 1;
                    }
                    Err(_) => {
                        held_since.get_or_insert(now);
                        skipped += 1;
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            (holds, gaps, blocks, skipped)
        })
    };

    std::thread::sleep(Duration::from_millis(300));
    let open_start = Instant::now();
    let mut window = PluginWindow::new(plugin.clone());
    let opened = window.open();
    let open_end = Instant::now();
    std::thread::sleep(Duration::from_millis(300));
    // Drag the window bigger, as a user would: a resizable editor follows it (as far as it
    // allows), a fixed one keeps its size. The audio thread must not wait for either.
    let editor = |plugin: &Arc<Mutex<vst3_host::Plugin>>| {
        let p = plugin.lock().unwrap_or_else(|e| e.into_inner());
        (
            p.attached_editor_size().ok().flatten(),
            p.attached_editor_can_resize(),
        )
    };
    // What the app does every frame: an editor's own requests made while opening are applied
    // (some ask for their size then).
    let _ = window.service_platform_events();
    let (before, resizable) = editor(&plugin);
    let resize_start = Instant::now();
    if let Some((w, h)) = before {
        window.drag_to(w + 160, h + 120);
        let _ = window.service_platform_events();
        // As the app's event loop does: an editor that applies the size on its next message
        // (JUCE's) gets it, and a frame later the window follows anything it asked for.
        PluginWindow::pump_messages(Duration::from_millis(300));
        let _ = window.service_platform_events();
    }
    let resize_end = Instant::now();
    let (after, _) = editor(&plugin);
    let window_size = window.client_size();
    std::thread::sleep(Duration::from_millis(200));
    let close_start = Instant::now();
    window.close();
    let close_end = Instant::now();
    std::thread::sleep(Duration::from_millis(200));
    running.store(false, Ordering::Relaxed);
    let (holds, gaps, blocks, skipped) = audio
        .join()
        .map_err(|_| anyhow!("the audio thread panicked"))?;
    opened.map_err(|e| anyhow!("{name}: opening the editor: {e}"))?;

    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    // The longest of `spans` that began within [from, to] (plus a turn's slack).
    let longest = |spans: &[(Instant, Duration)], from: Instant, to: Instant| {
        spans
            .iter()
            .filter(|(at, _)| *at + Duration::from_millis(2) >= from && *at <= to)
            .map(|s| s.1)
            .max()
            .unwrap_or_default()
    };
    let hold = holds.iter().map(|h| h.1).max().unwrap_or_default();
    let gap = gaps.iter().map(|g| g.1).max().unwrap_or_default();
    println!(
        "{name}: editor opened in {:.1} ms (lock held up to {:.1} ms, loop gap up to {:.1} ms), \
         closed in {:.1} ms (lock {:.1} ms, gap {:.1} ms); overall longest hold {:.1} ms, gap \
         {:.1} ms; {blocks} turns processed, {skipped} found the lock taken",
        ms(open_end - open_start),
        ms(longest(&holds, open_start, open_end)),
        ms(longest(&gaps, open_start, open_end)),
        ms(close_end - close_start),
        ms(longest(&holds, close_start, close_end)),
        ms(longest(&gaps, close_start, close_end)),
        ms(hold),
        ms(gap)
    );
    println!(
        "dragged the window 160 x 120 bigger: the editor went from {before:?} to {after:?} \
         ({}), the window's inside is {window_size:?}; in {:.1} ms, lock held up to {:.1} ms",
        match resizable {
            Some(true) => "resizable",
            Some(false) => "fixed size",
            None => "no answer",
        },
        ms(resize_end - resize_start),
        ms(longest(&holds, resize_start, resize_end)),
    );
    let mut failures = Vec::new();
    let block_ms = BLOCK as f64 / RATE * 1000.0;
    if ms(hold) >= block_ms {
        failures.push(format!(
            "opening, resizing or closing the editor held the plug-in for {:.1} ms (a block is \
             {block_ms:.0} ms): its track would go silent",
            ms(hold)
        ));
    }
    // Whatever the editor took (all of it, the nearest size it allows, or none of it), the
    // window fits it: no black space beside it.
    match (before, after, resizable) {
        (Some(b), Some(a), Some(false)) if a != b => {
            failures.push(format!("a fixed-size editor changed from {b:?} to {a:?}"))
        }
        (None, _, _) | (_, None, _) => failures.push("the editor gave no size".into()),
        (_, Some(a), _) if window_size != Some(a) => failures.push(format!(
            "the window's inside is {window_size:?} around an editor of {a:?}"
        )),
        _ => {}
    }
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("editor check failed")
    }
}
