//! The manual's marks (dev plan 27.2): where the named parts of the window were drawn, so the
//! manual's figures can outline them on the screenshot taken from the same frame.
//!
//! The UI calls [`put`] as it draws a part the manual points at (`marks::put(ui,
//! "transport.record", response.rect)`). Off (every normal run) that is one relaxed atomic load
//! and nothing else: no allocation, no lock. `--marks PATH` (with `--screenshot`) turns it on;
//! the frame the screenshot is taken from then writes its marks beside the PNG, in the image's
//! pixels, as the manual's generator reads them (`crates/manual`, `figure::Marks`):
//! `{"image": [w, h], "marks": {"transport.record": [x, y, w, h], ...}}`.
//!
//! Names are dotted, area first (`transport.`, `status.`, `menu.`, `dock.tab.`, `pane.`,
//! `track.`, `inspector.`, `mixer.strip.`, `mixer.master.`, `settings.audio.`,
//! `piano_roll.`, `waveform.rack.`, `cpu.`); `docs/manual/README.md` lists them. A part drawn
//! for every track keeps one: the track header's and the mixer strip's parts are the selected
//! track's (the others are drawn [`quiet`]); otherwise the first one of a name put in a frame
//! wins. Rects are clipped to what the `Ui` shows (a scrolled-away part leaves no mark), but
//! not to what covers them: a window over a part leaves its mark (pick shots accordingly).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use egui::{Rect, Ui};

/// Whether this run records marks (`--marks`).
static ON: AtomicBool = AtomicBool::new(false);

/// This frame's marks, in points, in the order they were put (first one of a name wins).
static FRAME: Mutex<Vec<(String, Rect)>> = Mutex::new(Vec::new());

/// Record marks from now on (`--marks`).
pub fn turn_on() {
    ON.store(true, Ordering::Relaxed);
}

/// Drawing that puts no marks (a track header but the selected one's), nested.
static QUIET: AtomicUsize = AtomicUsize::new(0);

/// Whether marks are being recorded.
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Whether a mark put now is kept: on, and not inside a [`quiet`] stretch.
fn keeping() -> bool {
    on() && QUIET.load(Ordering::Relaxed) == 0
}

/// While the guard lives (and `when`), marks put are ignored: the parts of a row drawn once
/// per track are marked only for the one the manual means.
pub fn quiet(when: bool) -> Quiet {
    let counted = when && on();
    if counted {
        QUIET.fetch_add(1, Ordering::Relaxed);
    }
    Quiet(counted)
}

/// See [`quiet`].
#[must_use]
pub struct Quiet(bool);

impl Drop for Quiet {
    fn drop(&mut self) {
        if self.0 {
            QUIET.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// A new frame: forget the last one's marks (`DawApp::ui`, before anything is drawn).
pub fn begin_frame() {
    if on() {
        lock().clear();
    }
}

/// `name` was drawn at `rect` (points), as far as `ui` shows it.
pub fn put(ui: &Ui, name: &'static str, rect: Rect) {
    if keeping() {
        add(name.to_string(), rect.intersect(ui.clip_rect()));
    }
}

/// [`put`] with a name made only when marks are recorded (a numbered slot, a section's row).
pub fn put_with(ui: &Ui, name: impl FnOnce() -> String, rect: Rect) {
    if keeping() {
        add(name(), rect.intersect(ui.clip_rect()));
    }
}

/// [`put`] for a rect not drawn in a `Ui` at hand (a window's, a panel's): not clipped.
pub fn put_rect(name: &'static str, rect: Rect) {
    if keeping() {
        add(name.to_string(), rect);
    }
}

/// [`put_rect`] with a name made only when marks are recorded.
pub fn put_rect_with(name: impl FnOnce() -> String, rect: Rect) {
    if keeping() {
        add(name(), rect);
    }
}

fn add(name: String, rect: Rect) {
    if !rect.is_positive() || !rect.is_finite() {
        return;
    }
    let mut frame = lock();
    if !frame.iter().any(|(n, _)| *n == name) {
        frame.push((name, rect));
    }
}

fn lock() -> std::sync::MutexGuard<'static, Vec<(String, Rect)>> {
    // Only the UI thread puts marks; a panic while holding the lock leaves a list that is
    // still a list.
    FRAME.lock().unwrap_or_else(|e| e.into_inner())
}

/// This frame's marks so far (the screenshot's, taken once the frame is drawn).
pub fn taken() -> Vec<(String, Rect)> {
    if on() {
        lock().clone()
    } else {
        Vec::new()
    }
}

/// The marks file for a screenshot `size` pixels big taken at `pixels_per_point`: each rect
/// in whole pixels (grown outwards to them), cut to the image; marks wholly outside it are
/// left out.
pub fn json(marks: &[(String, Rect)], pixels_per_point: f32, size: [usize; 2]) -> String {
    let [w, h] = size.map(|s| s as f32);
    let mut out: BTreeMap<&str, [u32; 4]> = BTreeMap::new();
    for (name, rect) in marks {
        let x0 = (rect.min.x * pixels_per_point).floor().clamp(0.0, w);
        let y0 = (rect.min.y * pixels_per_point).floor().clamp(0.0, h);
        let x1 = (rect.max.x * pixels_per_point).ceil().clamp(0.0, w);
        let y1 = (rect.max.y * pixels_per_point).ceil().clamp(0.0, h);
        if x1 > x0 && y1 > y0 {
            out.entry(name)
                .or_insert([x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32]);
        }
    }
    // A mark a line, so the file reads (and diffs) by name.
    let lines: Vec<String> = out
        .iter()
        .map(|(name, [x, y, w, h])| {
            let name = serde_json::to_string(name).unwrap_or_default();
            format!("    {name}: [{x}, {y}, {w}, {h}]")
        })
        .collect();
    format!(
        "{{\n  \"image\": [{}, {}],\n  \"marks\": {{\n{}\n  }}\n}}\n",
        size[0],
        size[1],
        lines.join(",\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, Rect};

    /// Marks are process-wide: the tests that turn them on run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn frame(ctx: &egui::Context, add: impl FnMut(&mut Ui)) {
        let mut add = add;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| add(ui));
        // Fonts were made: a test has no painter to hand them to.
        output.textures_delta.clear();
    }

    #[test]
    fn off_nothing_is_kept_and_put_is_a_load() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        ON.store(false, Ordering::Relaxed);
        lock().clear();
        let ctx = egui::Context::default();
        let mut made = false;
        begin_frame();
        frame(&ctx, |ui| {
            put(
                ui,
                "transport.record",
                Rect::from_min_size(pos2(1.0, 2.0), egui::vec2(3.0, 4.0)),
            );
            put_with(
                ui,
                || {
                    made = true;
                    "waveform.rack.slot.1".into()
                },
                Rect::from_min_size(pos2(1.0, 2.0), egui::vec2(3.0, 4.0)),
            );
        });
        assert!(!made, "a name is never made while marks are off");
        assert!(lock().is_empty());
        assert!(taken().is_empty());
    }

    #[test]
    fn on_the_first_of_a_name_is_kept_clipped_to_the_ui() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        turn_on();
        begin_frame();
        let ctx = egui::Context::default();
        frame(&ctx, |ui| {
            let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 50.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(clip), |ui| {
                ui.set_clip_rect(clip);
                put(
                    ui,
                    "track.mute",
                    Rect::from_min_max(pos2(90.0, 10.0), pos2(120.0, 20.0)),
                );
                put(
                    ui,
                    "track.mute",
                    Rect::from_min_max(pos2(0.0, 30.0), pos2(10.0, 40.0)),
                );
                put(
                    ui,
                    "gone",
                    Rect::from_min_max(pos2(200.0, 10.0), pos2(220.0, 20.0)),
                );
            });
        });
        // Only this test's names: other tests may draw while marks are on.
        let marks: Vec<(String, Rect)> = taken()
            .into_iter()
            .filter(|(n, _)| n == "track.mute" || n == "gone")
            .collect();
        ON.store(false, Ordering::Relaxed);
        lock().clear();
        assert_eq!(
            marks,
            vec![(
                "track.mute".to_string(),
                Rect::from_min_max(pos2(90.0, 10.0), pos2(100.0, 20.0))
            )]
        );
    }

    #[test]
    fn a_quiet_stretch_keeps_nothing() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        turn_on();
        begin_frame();
        let ctx = egui::Context::default();
        let rect = Rect::from_min_max(pos2(1.0, 1.0), pos2(5.0, 5.0));
        frame(&ctx, |ui| {
            {
                let _quiet = quiet(true);
                put(ui, "track.mute", rect);
                let _loud = quiet(false);
                put(ui, "track.solo", rect);
            }
            let _loud = quiet(false);
            put(ui, "track.arm", rect);
        });
        let names: Vec<String> = taken()
            .into_iter()
            .map(|(n, _)| n)
            .filter(|n| ["track.mute", "track.solo", "track.arm"].contains(&n.as_str()))
            .collect();
        ON.store(false, Ordering::Relaxed);
        lock().clear();
        assert_eq!(names, vec!["track.arm".to_string()]);
    }

    #[test]
    fn the_file_is_in_pixels_grown_to_whole_ones_and_cut_to_the_image() {
        let marks = vec![
            (
                "transport.record".to_string(),
                Rect::from_min_max(pos2(10.2, 5.0), pos2(30.0, 25.5)),
            ),
            (
                "status".to_string(),
                Rect::from_min_max(pos2(0.0, 590.0), pos2(1000.0, 610.0)),
            ),
            (
                "outside".to_string(),
                Rect::from_min_max(pos2(2000.0, 0.0), pos2(2100.0, 10.0)),
            ),
        ];
        let text = json(&marks, 1.5, [1200, 900]);
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["image"], serde_json::json!([1200, 900]));
        // 10.2 * 1.5 = 15.3 → 15; 30 * 1.5 = 45; 5 * 1.5 = 7.5 → 7; 25.5 * 1.5 = 38.25 → 39.
        assert_eq!(
            value["marks"]["transport.record"],
            serde_json::json!([15, 7, 30, 32])
        );
        // 590 * 1.5 = 885, cut at the image's 900 rows; 1000 * 1.5 = 1500 cut at 1200.
        assert_eq!(
            value["marks"]["status"],
            serde_json::json!([0, 885, 1200, 15])
        );
        assert!(value["marks"].get("outside").is_none());
        assert_eq!(
            value.as_object().map(|o| o.len()),
            Some(2),
            "only image and marks"
        );
    }
}
