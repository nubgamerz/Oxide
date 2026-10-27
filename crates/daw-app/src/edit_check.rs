//! Driven checks of the fields typed into (dev plan 22.1, 22.2) and of the Record button's
//! count-in (22.3), clicked and typed as the user would, the events fed in through
//! `raw_input_hook`:
//!
//! - `--show rename`: Guitar L's name in its header double-clicked, "Lead guitar" typed (the
//!   shot `ui-rename`, the field open), Enter; then in the inspector a blank name refused on
//!   Enter and Escaped, "Rhythm" typed and committed by clicking away; Escape in the header
//!   keeping the name; the Reverb bus renamed; Guitar R selected while a name is typed in the
//!   inspector, Guitar L keeping it. Each time the project's name, the header's and
//!   the inspector's text and the undo list are checked.
//! - `--show volume-edit`: Guitar L's volume readout beside its header's knob clicked, "-6.5 dB"
//!   typed (the shot `ui-volume-edit`), Enter; "loud" refused and Escaped; "-inf" and "+30"
//!   clamped to the fader's ends; its pan typed as "L30"; the inspector's volume typed and
//!   committed by clicking away; the master's volume typed. One undo step per typed value.
//! - `--show record-menu`: the Record button right-clicked (the shot `ui-record-menu`, its
//!   Count-in menu open) and "1 bar" picked: the metronome's `count_in_bars` is 1, as
//!   `set_metronome` sets it, in one "Metronome" undo step.
//! - `--show count-in`: the same, with the demo's Vocal armed on a simulated input
//!   (`start_virtual_input`, nothing heard) and the playhead on bar 2; then Record left-clicked:
//!   the time display shows the count-in ("Count-in · N", the shot `ui-count-in`) with the
//!   session's beats left, then the position once the take starts; the take starts at the
//!   playhead, and lands there as a clip when Record is clicked again.
//! - `--show clip-copy-paste` (24.1): Guitar L's clip clicked (it and Guitar L selected), Ctrl+C
//!   sent as egui-winit sends it (`Event::Copy`, never a `Key` with Ctrl, which is how the bug
//!   hid), Vocal's name clicked, the ruler clicked (the playhead there, Vocal still selected),
//!   Ctrl+V sent as `Event::Paste` with the line the copy put on the clipboard: the copy is on
//!   Vocal at the playhead, the original's in every other way, the selection, one "Paste
//!   clips" undo step (the shot `ui-clip-copy-paste`).
//! - `--show clip-edges` (25.1): the user's case: Guitar L's clip with a copy of it on the same
//!   track starting halfway along (drawn after it, so the left clip's end lies under it), the left
//!   one selected, Snap off. The pointer on the hidden end lights it (the shot `ui-clip-edges`);
//!   pressed there, the press takes that edge and changes nothing; dragged 40 points left, the
//!   left clip's end (and nothing else in the project) follows the pointer to the sample, in one
//!   "Trim clip" step. Then Snap on: the edge pressed and held without a move changes nothing
//!   and adds no step; moved 3 points, it goes onto the 1/16 grid.
//! - `--show clip-playhead` (25.2): Guitar L's clip on screen, nothing selected, a time
//!   selection over its start, the playhead at 0, Snap off on a 1/16 grid. A click inside the
//!   clip puts the playhead on the sample clicked, selects the clip and Guitar L and clears the
//!   time selection; Snap on, a click further along puts it on the grid line nearest (the shot
//!   `ui-clip-playhead`); a click on Keys' MIDI clip does the same there. Then a drag of Guitar
//!   L's clip moves the clip and not the playhead; a click on its fade-out handle and on its
//!   end (hovered first, lit) and a right-click on its body leave the playhead where it was.
//!   Then, Snap off, double-clicks as a mouse makes them (26.2: the second press once the
//!   playhead is drawn under the pointer): on Keys' MIDI clip the piano roll opens on it, and
//!   on Guitar L's clip the Waveform view opens on its file, the playhead at the click both
//!   times.
//! - `--show track-zoom` (26.1): every track 200 points tall, the rows scrolled down three
//!   wheel notches, the pointer over the track 60 % down the view; Alt held (the check sends
//!   its modifiers every frame) and three Alt+wheel notches sent as egui-winit sends them
//!   (`MouseWheel` in lines, carrying Alt): the rows grow (the shot `ui-track-zoom`), then three
//!   shrink them back; then Alt let go a frame into a notch, whose rest still grows them. On
//!   every frame from the first notch the track's row must have the pointer the same share
//!   down it as when it was noted, within a point.
//! - `--show playhead-drag` (26.3): zoomed in (8 samples a point) on a 1/8 grid with Snap on,
//!   the view 22,050 samples in: the playhead's handle pressed, the pointer thrown far past the
//!   lanes' right edge and out of the window in one frame and held there (the shot), then gone
//!   as Windows' mouse-leave message makes it (egui's `PointerGone`, the button still down),
//!   then thrown far past the left edge and held until the view is back at the start, and let
//!   go; then Snap off, thrown past the right edge and let go out there. On every frame of
//!   each drag egui must still be dragging the handle, the playhead must be drawn in the lanes,
//!   and the view must be scrolling the pointer's way (stopping at the start); let go, the drag
//!   is over, the view still and the playhead on screen where it was drawn.
//! - `--show piano-roll-click` (26.4): the piano roll open on Keys' clip, a note selected, the
//!   editor's Snap on. A click on empty grid puts the playhead on the grid line nearest it and
//!   clears the selection; Snap off, a click further along puts it on the tick clicked; a
//!   click in a note's middle puts it there and selects the note; that note dragged from the
//!   same spot (the playhead's line under the pointer) moves the note and not the playhead;
//!   a double-click on empty grid as a mouse makes it (the second press once the line is
//!   drawn under the pointer) draws a note there, the playhead at the click (the shot); a
//!   click on a key of the keyboard and on the velocity lane leave the playhead where it was.
//! - `--show help-manual` (27.3): Help clicked, its menu open with Manual (the shot
//!   `ui-help-menu`); Manual clicked asks for the manual's first page; the piano roll opened on
//!   Keys' clip and F1 pressed over it asks for its page; Options > Settings… on Audio and F1
//!   over the window asks for `settings#audio`. A check run never starts a browser: each
//!   request is noted in `DawApp::manual_requests`, and the check fails unless they are those
//!   three, in order, and F1's pages are `help::Place::page`'s.
//!
//! They print `RESULT: PASS` or `FAIL:` lines and the app exits 1 on a failure.

use crate::app::{DawApp, Selection};
use crate::value_edit::{self, Level, Place};
use daw_core::midi_clip::{Note, NoteId};
use daw_core::project::{Clip, ClipId, TrackId, TrackKind};
use egui::{Event, Id, Key, Modifiers, PointerButton};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static FAILED: AtomicBool = AtomicBool::new(false);

/// The check run failed: the app exits 1 (`main`).
pub fn failed() -> bool {
    FAILED.load(Ordering::Relaxed)
}

/// Frames before the first click (the window up and settled).
const START: u32 = 30;
/// Frames an input step waits on its own step first: a field opened by the last step asks for
/// focus as it is first drawn, so keys sent sooner would go nowhere.
const SETTLE: u32 = 3;
/// Frames the shot's step holds after the screenshot is asked for.
const HOLD_SHOT: u32 = 5;
/// How long after a click the next click or double-click starts: egui takes two clicks
/// within 0.3 s for a double-click, and a third within 0.6 s for a triple-click.
const CLICKS_APART: Duration = Duration::from_millis(800);
/// Frames any one step may wait before the check gives up on it.
const TIMEOUT: u32 = 300;
/// How long a `Wait` may wait: it waits on the transport (a count-in, a take), which runs in
/// real time however fast frames come.
const WAIT_TIMEOUT: Duration = Duration::from_secs(20);

/// Which check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Rename,
    Volume,
    /// The Record button's count-in menu (22.3).
    RecordMenu,
    /// The same, then a take recorded through a count-in.
    CountIn,
    /// A clip copied with Ctrl+C and pasted on another track at the playhead with Ctrl+V
    /// (24.1).
    ClipCopyPaste,
    /// A clip's end under another clip grabbed and dragged; a press without a move (25.1).
    ClipEdges,
    /// A click on a clip places the playhead; a drag, a handle, an edge or a right-click
    /// don't (25.2).
    ClipPlayhead,
    /// Alt+wheel grows and shrinks the track heights around the pointer (26.1).
    TrackZoom,
    /// The playhead dragged fast past either edge, zoomed in: the view follows (26.3).
    PlayheadDrag,
    /// A click in the piano roll's note grid places the playhead; a double-click still draws,
    /// a note's drag, the keys and the velocity lane leave it (26.4).
    PianoRollClick,
    /// Help > Manual clicked, F1 over the piano roll and in Settings > Audio: the pages asked
    /// for, and no browser started (27.3).
    HelpManual,
}

/// A check failed before it could start (the app exits 1).
pub fn fail() {
    FAILED.store(true, Ordering::Relaxed);
}

type Test = fn(&DawApp, &egui::Context, &Check) -> Result<(), String>;
/// Where a `ClickAt` clicks, worked out from what was drawn last frame.
type Spot = fn(&DawApp, &Check) -> Option<egui::Pos2>;

enum Step {
    /// Click at a place found by the function (a clip, the ruler: no field of their own).
    ClickAt(&'static str, Spot),
    /// Right-click there.
    RightClickAt(&'static str, Spot),
    /// Double-click there as a mouse does (26.2): the pointer moved there, pressed in one frame
    /// and let go in the next; then, once the playhead has been drawn where that click put it
    /// (a real second press comes 100-200 ms later, after the engine has taken the locate), the
    /// second press and release, all inside egui's double-click time.
    DoubleClickAt(&'static str, Spot),
    /// Send these events, as egui-winit would (the clipboard's `Event::Copy` and `Paste`).
    Send(&'static str, fn(&DawApp) -> Vec<Event>),
    /// Move the pointer there, its button up or held (a hover, or a drag's next point).
    PointerTo(&'static str, Spot),
    /// Press the primary button there (the project's clips noted, as `held`), and keep it down.
    PointerDown(&'static str, Spot),
    /// Let it go where the pointer is.
    PointerUp,
    /// Change something in the app, as a setting the user flips (Snap).
    Do(&'static str, fn(&mut DawApp)),
    /// Click what was drawn under this id (`value_edit::remember`).
    Click(Id),
    /// Right-click it.
    RightClick(Id),
    DoubleClick(Id),
    Type(&'static str),
    Press(Key),
    /// Wait for the field to be open (`true`) or closed.
    Open(Id, bool),
    /// The screenshot, the field as it is.
    Shot,
    Expect(&'static str, Test),
    /// Wait until the test passes (the failure seen last is reported if it never does).
    Wait(&'static str, Test),
}

pub struct Check {
    pub kind: Kind,
    /// Guitar L.
    track: TrackId,
    /// The Reverb bus.
    bus: Option<TrackId>,
    /// Guitar R, selected while Guitar L's name is typed in the inspector.
    other: Option<TrackId>,
    steps: Vec<Step>,
    at: usize,
    /// Frames on the current step, and when it began.
    waited: u32,
    step_started: Instant,
    frame: u32,
    /// The undo list's length before the first step.
    undo_before: usize,
    shot_at: Option<u32>,
    /// A double-click's first click went in at this frame.
    first_click: Option<u32>,
    /// When the last click went in.
    clicked_at: Option<Instant>,
    /// No screenshot asked for this run: the shot's step passes straight on.
    no_shot: bool,
    /// The count-in check: where the take is to start (the playhead before Record, bar 2), and
    /// the clips on the armed track then.
    record_point: u64,
    clips_before: usize,
    /// The copy-and-paste check: Guitar L's clip, and Vocal, where its copy is pasted. The
    /// edges check: the left clip (Guitar L's), and `cover`, its copy over its end.
    clip: Option<ClipId>,
    target: Option<TrackId>,
    cover: Option<ClipId>,
    /// The edges check: every clip in the project as the check began, and as the last press
    /// (or click) went in; where that press was, and where the pointer is now.
    clips_then: Vec<Clip>,
    held: Vec<Clip>,
    grab: Option<egui::Pos2>,
    pointer: Option<egui::Pos2>,
    /// The playhead check: where the last click (or right-click) went in, the playhead as the
    /// last click or press went in, and Keys' MIDI clip with its track.
    clicked: Option<egui::Pos2>,
    position_before: u64,
    midi: Option<(TrackId, ClipId)>,
    /// Samples a point, as the check set it.
    spp: f64,
    /// A `DoubleClickAt` under way.
    double: Option<Double>,
    /// The playhead a `DoubleClickAt`'s click is to put it on, worked out as it went in.
    expected: Option<u64>,
    /// The modifiers the check holds down, sent every frame (Alt for the track zoom).
    modifiers: Modifiers,
    /// The track zoom check: the row under the pointer, watched every frame while it's set.
    zoom: Option<ZoomWatch>,
    /// The track zoom check: that track's height as Alt was let go mid-notch.
    released_height: f32,
    /// The playhead drag check: the drag watched every frame while it's set; where the pointer
    /// was as it went (`PointerGone` sent once, then nothing, until the next `PointerTo`), and
    /// the view's start as last noted.
    drag: Option<DragWatch>,
    gone: Option<egui::Pos2>,
    gone_sent: bool,
    noted_view: f64,
    /// The piano roll check: the clip's notes as noted last, and the note picked to click
    /// and drag.
    notes_then: Vec<Note>,
    piano_note: Option<NoteId>,
    /// A `Wait`'s last failure, reported if it times out.
    waiting: Option<String>,
    failures: Vec<String>,
    reported: bool,
}

/// The check `kind` on the demo: Guitar L selected, the readouts' places kept. `shot`: a
/// screenshot is asked for (`--screenshot`).
pub fn start(app: &mut DawApp, kind: Kind, shot: bool) -> Option<Check> {
    let find = |name: &str| {
        app.session
            .project
            .tracks
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.id)
    };
    let track = find("Guitar L")?;
    let bus = app
        .session
        .project
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Bus)
        .map(|t| t.id);
    app.selection = Some(Selection::Track(track));
    value_edit::remember_places();
    let steps = match kind {
        Kind::Rename => rename_steps(track, bus, find("Guitar R")),
        Kind::Volume => volume_steps(track),
        Kind::RecordMenu | Kind::CountIn => count_in_steps(kind),
        Kind::ClipCopyPaste => copy_paste_steps(find("Vocal")?),
        Kind::ClipEdges => edge_steps(),
        Kind::ClipPlayhead => playhead_steps(),
        Kind::TrackZoom => track_zoom_steps(),
        Kind::PlayheadDrag => playhead_drag_steps(),
        Kind::PianoRollClick => piano_click_steps(),
        Kind::HelpManual => help_steps(),
    };
    if kind == Kind::ClipCopyPaste {
        // Nothing selected: the clip's click must select it and its track.
        app.selection = None;
        app.selected_clip = None;
        app.selected_clips.clear();
    }
    let target = find("Vocal");
    let clips_before = match kind {
        Kind::ClipCopyPaste => app
            .session
            .project
            .track(target?)
            .map_or(0, |t| t.clips.len()),
        _ => armed(app).map_or(0, |t| t.clips.len()),
    };
    let clip = app
        .session
        .project
        .track(track)
        .and_then(|t| t.clips.first())
        .map(|c| c.id);
    let other = find("Guitar R");
    let keys = find("Keys");
    let cover = if kind == Kind::ClipEdges {
        Some(overlap(app, track, clip?)?)
    } else {
        None
    };
    let midi = match kind {
        Kind::ClipPlayhead => {
            let keys = keys?;
            let midi = app.session.project.track(keys)?.midi_clips.first()?.id;
            playhead_layout(app, track, clip?)?;
            Some((keys, midi))
        }
        Kind::PianoRollClick => Some(piano_click_layout(app, keys?)?),
        _ => None,
    };
    if kind == Kind::TrackZoom {
        track_zoom_layout(app);
    }
    if kind == Kind::PlayheadDrag {
        playhead_drag_layout(app);
    }
    Some(Check {
        kind,
        track,
        bus,
        other,
        steps,
        at: 0,
        waited: 0,
        step_started: Instant::now(),
        frame: 0,
        undo_before: app.session.history().0.len(),
        shot_at: None,
        first_click: None,
        clicked_at: None,
        no_shot: !shot,
        // Bar 2, where `DawApp::virtual_input_demo` put the playhead (the engine takes the
        // locate at its next block, so `position()` may not say so yet).
        record_point: app.session.tempo_map().bar_start(1).round() as u64,
        clips_before,
        clip,
        target,
        cover,
        clips_then: all_clips(app),
        held: Vec::new(),
        grab: None,
        pointer: None,
        clicked: None,
        position_before: 0,
        midi,
        double: None,
        expected: None,
        spp: app.samples_per_pixel,
        modifiers: Modifiers::NONE,
        zoom: None,
        released_height: 0.0,
        drag: None,
        gone: None,
        gone_sent: false,
        noted_view: 0.0,
        notes_then: Vec::new(),
        piano_note: None,
        waiting: None,
        failures: Vec::new(),
        reported: false,
    })
}

/// The count-in check's armed track: the demo's Vocal (`DawApp::virtual_input_demo`).
fn armed(app: &DawApp) -> Option<&daw_core::project::Track> {
    let vocal = app
        .session
        .project
        .tracks
        .iter()
        .find(|t| t.name == "Vocal");
    vocal.filter(|t| t.arm)
}

/// The Record button's count-in menu, and for `CountIn` a take recorded through the count-in.
fn count_in_steps(kind: Kind) -> Vec<Step> {
    let record = crate::transport::record_button_id();
    let one = crate::transport::count_in_item_id(1);
    let mut steps = vec![
        Step::RightClick(record),
        Step::Wait("the Record button's menu opened", |_, ctx, _| {
            if !crate::transport::record_menu_open(ctx) {
                return Err("no menu".into());
            }
            let item = value_edit::drawn(ctx, crate::transport::count_in_item_id(1));
            same(item.map(|d| d.text).as_deref(), Some("1 bar"))
        }),
    ];
    if kind == Kind::RecordMenu {
        // Half a second for the menu to fade in before the shot.
        steps.push(Step::Wait("the menu stayed open", |_, ctx, c| {
            if !crate::transport::record_menu_open(ctx) {
                return Err("it closed".into());
            }
            if c.step_started.elapsed() < Duration::from_millis(500) {
                return Err("not half a second yet".into());
            }
            Ok(())
        }));
        steps.push(Step::Shot);
    }
    steps.extend([
        Step::Click(one),
        Step::Wait("the menu closed", |_, ctx, _| {
            if crate::transport::record_menu_open(ctx) {
                Err("still open".into())
            } else {
                Ok(())
            }
        }),
        Step::Expect(
            "1 bar picked: the metronome's count-in, as set_metronome sets it, one step",
            |app, _, c| {
                let bars = app.session.project.metronome.count_in_bars;
                if bars != 1 {
                    return Err(format!("count_in_bars is {bars}, not 1"));
                }
                undo_steps(app, c, 1, "Metronome")
            },
        ),
    ]);
    if kind == Kind::RecordMenu {
        return steps;
    }
    steps.extend([
        Step::Expect("Vocal is armed on the simulated input", |app, _, _| {
            if armed(app).is_none() || app.session.input_channels() == 0 {
                return Err("no armed track, or no input".into());
            }
            Ok(())
        }),
        // A left click still records.
        Step::Click(record),
        Step::Wait(
            "the count-in shows in the time display, with the beats left",
            |app, ctx, _| {
                let Some(count_in) = app.session.count_in_progress() else {
                    return Err("no count-in in progress".into());
                };
                let (big, _) = crate::transport::count_in_text(&count_in);
                same(
                    drawn(ctx, crate::transport::clock_id()).as_deref(),
                    Some(big.as_str()),
                )?;
                if count_in.bars != 1 {
                    return Err(format!("a count-in of {} bars", count_in.bars));
                }
                Ok(())
            },
        ),
        Step::Shot,
        Step::Wait(
            "the take started: the display shows the position",
            |app, ctx, _| {
                if app.session.recording_takes().is_empty() {
                    return Err("no take yet".into());
                }
                let shown = drawn(ctx, crate::transport::clock_id()).unwrap_or_default();
                if shown.starts_with("Count-in") {
                    return Err(format!("the display still shows {shown:?}"));
                }
                Ok(())
            },
        ),
        Step::Expect("the take starts at the playhead", |app, _, c| {
            let takes = app.session.recording_takes();
            let starts: Vec<u64> = takes.iter().map(|t| t.start).collect();
            if starts != [c.record_point] {
                return Err(format!(
                    "the take starts at {starts:?}, the playhead was {}",
                    c.record_point
                ));
            }
            Ok(())
        }),
        // Recording again (Record toggles): stop.
        Step::Click(record),
        Step::Wait("the take became a clip at the playhead", |app, _, c| {
            if app.session.is_recording() {
                return Err("still recording".into());
            }
            let track = armed(app).ok_or("no armed track")?;
            if track.clips.len() != c.clips_before + 1 {
                return Err(format!(
                    "{} clips on {}, {} before",
                    track.clips.len(),
                    track.name,
                    c.clips_before
                ));
            }
            let starts: Vec<u64> = track.clips.iter().map(|clip| clip.start).collect();
            if !starts.contains(&c.record_point) {
                return Err(format!(
                    "no clip at the playhead ({}): {starts:?}",
                    c.record_point
                ));
            }
            Ok(())
        }),
    ]);
    steps
}

/// Guitar L's clip copied and pasted onto `vocal` at the playhead, with the clipboard's events.
fn copy_paste_steps(vocal: TrackId) -> Vec<Step> {
    vec![
        Step::ClickAt("Guitar L's clip", |app, c| {
            // Low in the clip, past its fade handle and away from the gain line and edges.
            let clip = c.clip?;
            let rect = app.clip_rects.iter().find(|(id, _)| *id == clip)?.1;
            let x = rect.left() + (rect.width() / 2.0).min(120.0);
            Some(egui::pos2(x, rect.bottom() - 8.0))
        }),
        Step::Expect("the click selected the clip and its track", |app, _, c| {
            let clip = c.clip.ok_or("Guitar L has no clip")?;
            if app.selected_clip_ids() != [clip] {
                return Err(format!("selected clips {:?}", app.selected_clip_ids()));
            }
            if app.selection != Some(Selection::Track(c.track)) {
                return Err(format!("the selection is {:?}", app.selection));
            }
            Ok(())
        }),
        Step::Send("Ctrl+C", |_| vec![Event::Copy]),
        Step::Expect(
            "Ctrl+C (Event::Copy) copied it, and put a line on the clipboard",
            |app, _, c| {
                if app.session.clipboard_len() != 1 {
                    return Err(format!(
                        "the clipboard holds {} clips",
                        app.session.clipboard_len()
                    ));
                }
                same(
                    app.clipboard_line.as_deref(),
                    Some("1 clip copied in Oxide"),
                )?;
                no_new_undo_steps(app, c).map_err(|e| format!("copying is no edit: {e}"))
            },
        ),
        Step::Click(value_edit::name_id(Place::Header, Some(vocal))),
        Step::Expect("Vocal's name clicked: Vocal is selected", |app, _, c| {
            let vocal = c.target.ok_or("no Vocal")?;
            if app.selection != Some(Selection::Track(vocal)) {
                return Err(format!("the selection is {:?}", app.selection));
            }
            Ok(())
        }),
        Step::ClickAt("the ruler", |app, _| {
            let ruler = app.ruler_rect?;
            Some(egui::pos2(
                ruler.left() + ruler.width() * 0.45,
                ruler.center().y,
            ))
        }),
        Step::Wait(
            "the ruler clicked: the playhead moved there, Vocal still selected",
            |app, _, c| {
                // Where the ruler's click puts it: the clip's click (25.2) already moved it off 0.
                playhead_on_click(app, c)?;
                if app.session.is_playing() {
                    return Err("playing".into());
                }
                let vocal = c.target.ok_or("no Vocal")?;
                if app.selection != Some(Selection::Track(vocal)) {
                    return Err(format!("the selection is {:?}", app.selection));
                }
                Ok(())
            },
        ),
        Step::Send("Ctrl+V", |app| {
            // What egui-winit sends while the clipboard holds the copy's line.
            let line = app.clipboard_line.clone();
            vec![Event::Paste(line.unwrap_or_else(|| "text".into()))]
        }),
        Step::Expect(
            "Ctrl+V (Event::Paste) put the copy on Vocal at the playhead, selected",
            |app, _, c| {
                let (clip, vocal) = (c.clip.ok_or("no clip")?, c.target.ok_or("no Vocal")?);
                let project = &app.session.project;
                let original = project.find_clip(clip).ok_or("the original is gone")?.1;
                if project.clip_track(clip) != Some(c.track) {
                    return Err("the original left Guitar L".into());
                }
                let track = project.track(vocal).ok_or("no Vocal")?;
                if track.clips.len() != c.clips_before + 1 {
                    return Err(format!(
                        "{} clips on Vocal, {} before",
                        track.clips.len(),
                        c.clips_before
                    ));
                }
                let copy = track.clips.last().expect("one more");
                let playhead = app.session.position();
                if copy.start != playhead {
                    return Err(format!(
                        "the copy starts at {}, the playhead is at {playhead}",
                        copy.start
                    ));
                }
                let mut moved = original.clone();
                moved.id = copy.id;
                moved.start = copy.start;
                if *copy != moved {
                    return Err("the copy differs from the original beyond id and start".into());
                }
                if app.selected_clip_ids() != [copy.id]
                    || app.selection != Some(Selection::Track(vocal))
                {
                    return Err(format!(
                        "selected {:?} on {:?}, not the copy on Vocal",
                        app.selected_clip_ids(),
                        app.selection
                    ));
                }
                undo_steps(app, c, 1, "Paste clips")
            },
        ),
        Step::Shot,
    ]
}

/// The edges check's layout (25.1): a copy of `clip` on its own track from halfway along it
/// (off the grid), drawn after it so it covers its end; the left clip selected, Snap off, a
/// 1/16 grid, the view showing both, the playhead at the start. Returns the copy.
fn overlap(app: &mut DawApp, track: TrackId, clip: ClipId) -> Option<ClipId> {
    let length = app.session.project.find_clip(clip)?.1.length;
    let delta = (length / 2 + 1_237) as i64;
    let copy = *app.session.copy_clips_to(&[clip], delta, 0).ok()?.first()?;
    let end = app.session.project.find_clip(copy)?.1.end();
    app.session.project.grid = daw_core::grid::Grid::Fixed(daw_core::grid::Division::note(16));
    app.settings.snap = false;
    app.view_start = 0.0;
    app.samples_per_pixel = (end as f64 / 900.0).max(1.0);
    let located = app.session.locate(0);
    app.report(located);
    app.select_clip(track, clip, false);
    Some(copy)
}

/// Every audio clip in the project, track by track.
fn all_clips(app: &DawApp) -> Vec<Clip> {
    let tracks = &app.session.project.tracks;
    tracks
        .iter()
        .flat_map(|t| t.clips.iter().cloned())
        .collect()
}

/// Low on the left clip's end, inside the copy covering it.
fn hidden_end(app: &DawApp, c: &Check) -> Option<egui::Pos2> {
    let clip = c.clip?;
    let rect = app.clip_rects.iter().find(|(id, _)| *id == clip)?.1;
    Some(egui::pos2(rect.right(), rect.bottom() - 8.0))
}

/// The left clip's end is lit as the edge under the pointer, and as hidden by its copy.
fn lit_hidden(app: &DawApp, c: &Check) -> Result<(), String> {
    let want = crate::arrange::LitEdge {
        clip: c.clip.ok_or("no clip")?,
        left: false,
        hidden: true,
    };
    if app.edge_lit != Some(want) {
        return Err(format!("the lit edge is {:?}", app.edge_lit));
    }
    Ok(())
}

/// The press took the left clip's end (not its copy's start or body), and selected that clip.
fn grabbed(app: &DawApp, c: &Check) -> Result<(), String> {
    let drag = app.edge_drag.as_ref().ok_or_else(|| {
        let moving = app.group_drag.as_ref().map(|d| d.grip);
        format!("no edge is being dragged (a clip drag of {moving:?})")
    })?;
    if Some(drag.clip) != c.clip || drag.left {
        return Err(format!(
            "dragging clip {}'s {} edge",
            drag.clip,
            if drag.left { "left" } else { "right" }
        ));
    }
    if app.selected_clip_ids() != [drag.clip] {
        return Err(format!("selected clips {:?}", app.selected_clip_ids()));
    }
    Ok(())
}

/// Every clip as it was in `then`.
fn unchanged(app: &DawApp, then: &[Clip]) -> Result<(), String> {
    let now = all_clips(app);
    if now.len() != then.len() {
        return Err(format!("{} clips, {} before", now.len(), then.len()));
    }
    match now.iter().zip(then).find(|(a, b)| a != b) {
        Some((now, then)) => Err(format!(
            "clip {} changed: {}..{} (was {}..{})",
            now.id,
            now.start,
            now.end(),
            then.start,
            then.end()
        )),
        None => Ok(()),
    }
}

/// The left clip's end grabbed under its copy and dragged; pressed and held with Snap on;
/// moved a little with it (25.1).
fn edge_steps() -> Vec<Step> {
    vec![
        Step::PointerTo("the left clip's end, under its copy", hidden_end),
        Step::Wait("the hidden end is lit", |app, _, c| lit_hidden(app, c)),
        Step::Shot,
        Step::PointerDown("the hidden end", hidden_end),
        Step::Expect(
            "the press took the left clip's end and changed nothing",
            |app, _, c| {
                grabbed(app, c)?;
                unchanged(app, &c.held)?;
                no_new_undo_steps(app, c)
            },
        ),
        Step::PointerTo("10 points left", |_, c| {
            c.grab.map(|g| g - egui::vec2(10.0, 0.0))
        }),
        Step::PointerTo("20 points left", |_, c| {
            c.grab.map(|g| g - egui::vec2(20.0, 0.0))
        }),
        Step::PointerTo("30 points left", |_, c| {
            c.grab.map(|g| g - egui::vec2(30.0, 0.0))
        }),
        Step::PointerTo("40 points left", |_, c| {
            c.grab.map(|g| g - egui::vec2(40.0, 0.0))
        }),
        Step::PointerUp,
        Step::Expect(
            "dragged 40 points left: the left clip's end followed (Snap off), nothing else \
             changed, one Trim clip step",
            |app, _, c| {
                let (clip, cover) = (c.clip.ok_or("no clip")?, c.cover.ok_or("no copy")?);
                let then = c
                    .clips_then
                    .iter()
                    .find(|k| k.id == clip)
                    .ok_or("no clip")?;
                let now = app.session.project.find_clip(clip).ok_or("gone")?.1;
                let want = then.end() as f64 - 40.0 * c.spp;
                if (now.end() as f64 - want).abs() > 2.0 {
                    return Err(format!(
                        "it ends at {}, wanted {want:.0} ({} before)",
                        now.end(),
                        then.end()
                    ));
                }
                let mut trimmed = then.clone();
                trimmed.length = now.length;
                if *now != trimmed {
                    return Err("the clip changed beyond its end".into());
                }
                let others: Vec<Clip> = c
                    .clips_then
                    .iter()
                    .map(|k| if k.id == clip { now.clone() } else { k.clone() })
                    .collect();
                unchanged(app, &others).map_err(|e| format!("another clip: {e}"))?;
                if app.edge_drag.is_some() {
                    return Err("the drag is still under way".into());
                }
                let copy = app
                    .session
                    .project
                    .find_clip(cover)
                    .ok_or("the copy's gone")?
                    .1;
                if !(copy.start < now.end() && now.end() < copy.end()) {
                    return Err("the end isn't under the copy any more".into());
                }
                undo_steps(app, c, 1, "Trim clip")
            },
        ),
        Step::Do("Snap on", |app| app.settings.snap = true),
        Step::PointerTo("the left clip's end again", hidden_end),
        Step::Wait("it is lit", |app, _, c| lit_hidden(app, c)),
        Step::PointerDown("the hidden end, Snap on", hidden_end),
        Step::Expect(
            "Snap on, pressed: the end taken, nothing changed",
            |app, _, c| {
                grabbed(app, c)?;
                unchanged(app, &c.held)?;
                undo_steps(app, c, 1, "Trim clip")
            },
        ),
        Step::Wait("held for half a second without a move", |_, _, c| {
            if c.step_started.elapsed() < Duration::from_millis(500) {
                return Err("holding".into());
            }
            Ok(())
        }),
        Step::Expect("held: still nothing changed", |app, _, c| {
            grabbed(app, c)?;
            unchanged(app, &c.held)?;
            undo_steps(app, c, 1, "Trim clip")
        }),
        Step::PointerUp,
        Step::Expect(
            "let go without a move: the project as it was, no new undo step",
            |app, _, c| {
                unchanged(app, &c.held)?;
                if app.edge_drag.is_some() {
                    return Err("the drag is still under way".into());
                }
                undo_steps(app, c, 1, "Trim clip")
            },
        ),
        Step::PointerTo("the left clip's end once more", hidden_end),
        Step::Wait("lit", |app, _, c| lit_hidden(app, c)),
        Step::PointerDown("the hidden end", hidden_end),
        Step::PointerTo("3 points left", |_, c| {
            c.grab.map(|g| g - egui::vec2(3.0, 0.0))
        }),
        Step::Expect(
            "moved 3 points with Snap on: the end on the 1/16 grid",
            |app, _, c| {
                grabbed(app, c)?;
                let clip = c.clip.ok_or("no clip")?;
                let then = c.held.iter().find(|k| k.id == clip).ok_or("no clip")?;
                let now = app.session.project.find_clip(clip).ok_or("gone")?.1;
                let end = now.end() as f64;
                let line = crate::arrange::snap(app, end);
                if now.end() == then.end() || line != end {
                    return Err(format!(
                        "it ends at {} ({} before), the grid's line there is {line}",
                        now.end(),
                        then.end()
                    ));
                }
                Ok(())
            },
        ),
        Step::PointerUp,
        Step::Expect("that drag is a Trim clip step", |app, _, c| {
            // Two drags of one clip's edge in quick succession merge into one step.
            let made = app.session.history().0.len().saturating_sub(c.undo_before);
            undo_steps(app, c, made.clamp(1, 2), "Trim clip")
        }),
    ]
}

/// The playhead check's layout (25.2): Guitar L's `clip` over the first 600 points of the
/// lanes (Keys' MIDI clip under it in time), a 1/16 grid with Snap off, the playhead at 0,
/// nothing selected and a time selection over the clip's first quarter on `track`, for the
/// click to clear.
fn playhead_layout(app: &mut DawApp, track: TrackId, clip: ClipId) -> Option<()> {
    let end = app.session.project.find_clip(clip)?.1.end();
    app.session.project.grid = daw_core::grid::Grid::Fixed(daw_core::grid::Division::note(16));
    app.settings.snap = false;
    app.view_start = 0.0;
    app.samples_per_pixel = (end as f64 / 600.0).max(1.0);
    let located = app.session.locate(0);
    app.report(located);
    app.selection = None;
    app.selected_clip = None;
    app.selected_clips.clear();
    app.time_selection = Some(daw_core::api::TimeSelection {
        start: 0,
        end: end / 4,
        tracks: vec![track],
    });
    Some(())
}

/// The track zoom check's demo: every track 200 points tall, so the rows scroll.
fn track_zoom_layout(app: &mut DawApp) {
    for track in &mut app.session.project.tracks {
        track.height = Some(200.0);
    }
}

/// A double-click under way: where, how far it has got (1 pressed, 2 let go, 3 pressed again),
/// when its first press went in and the frame of its last event.
struct Double {
    at: egui::Pos2,
    phase: u8,
    first: Instant,
    frame: u32,
}

/// How long after the first press a `DoubleClickAt`'s second press goes in at the soonest (a
/// hand's double-click: about 100-150 ms) and at the latest, well inside egui's 0.3 s
/// `max_double_click_delay`.
const SECOND_PRESS_FROM: Duration = Duration::from_millis(100);
const SECOND_PRESS_BY: Duration = Duration::from_millis(200);

/// The row a track zoom keeps under the pointer.
struct ZoomWatch {
    track: TrackId,
    /// Where the pointer is on screen, and how far down the track's own row, as a share of
    /// its height.
    y: f32,
    share: f32,
    /// The track's height when it was noted.
    height: f32,
    /// Frames watched, those the row was off by a point or more, and the most it was off.
    frames: u32,
    misses: u32,
    worst: f32,
}

/// The rows' scroll offset as drawn last frame: how far the master's row is above the view's
/// top.
fn rows_offset(app: &DawApp) -> Option<f32> {
    Some(app.track_view?.0.top() - app.master_row?.top())
}

/// A notch of the mouse wheel as egui-winit sends it on Windows (`LineDelta`, the modifiers
/// held), `lines` up (positive: Alt grows the rows, plain scrolls them up) or down.
fn notch(lines: f32, modifiers: Modifiers) -> Vec<Event> {
    vec![Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, lines),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers,
    }]
}

/// A notch's frames spread out (egui smooths a notch over about 0.2 s) and done.
fn notch_done(_: &DawApp, _: &egui::Context, c: &Check) -> Result<(), String> {
    if c.step_started.elapsed() < Duration::from_millis(500) {
        return Err("the notch still going".into());
    }
    Ok(())
}

/// The watched track's height now.
fn zoomed_height(app: &DawApp, c: &Check) -> Option<f32> {
    let w = c.zoom.as_ref()?;
    app.session.project.track(w.track).map(|t| t.height())
}

/// Every frame while the track zoom check watches: the noted track's row has the pointer the
/// same share down it as when it was noted, within a point.
fn watch_rows(app: &mut DawApp) {
    let Some(c) = &app.edit_check else {
        return;
    };
    let Some(w) = &c.zoom else {
        return;
    };
    let row = app.drop_rows.iter().find(|r| r.0 == w.track).map(|r| r.2);
    let height = app.session.project.track(w.track).map(|t| t.height());
    let step = c.describe();
    let c = app.edit_check.as_mut().expect("checked");
    let frame = c.frame;
    let Some(w) = &mut c.zoom else {
        return;
    };
    w.frames += 1;
    let (Some(row), Some(height)) = (row, height) else {
        c.failures
            .push(format!("{step}: the watched track's row was not drawn"));
        c.zoom = None;
        return;
    };
    let off = (row.top() + w.share * height - w.y).abs();
    w.worst = w.worst.max(off);
    if off >= 1.0 {
        w.misses += 1;
        if w.misses <= 3 {
            let line = format!(
                "{step}, frame {frame}: the row under the pointer moved {off:.1} points (its top \
                 at {:.1}, {height:.1} points tall)",
                row.top()
            );
            c.failures.push(line);
        }
    }
}

/// The pointer's track noted: the row watched from now on.
fn note_zoomed_row(app: &mut DawApp) {
    let Some(c) = &app.edit_check else {
        return;
    };
    let Some(y) = c.pointer.map(|p| p.y) else {
        return;
    };
    let project = &app.session.project;
    let under = app.drop_rows.iter().find_map(|(id, _, rect)| {
        let height = project.track(*id)?.height();
        (rect.top() <= y && y < rect.top() + height)
            .then(|| (*id, (y - rect.top()) / height, height))
    });
    let offset = rows_offset(app).unwrap_or(0.0);
    let c = app.edit_check.as_mut().expect("checked");
    match under {
        Some((track, share, height)) => {
            println!(
                "the pointer {:.0} % down a {height:.0}-point row, the rows {offset:.0} points \
                 down",
                share * 100.0
            );
            c.zoom = Some(ZoomWatch {
                track,
                y,
                share,
                height,
                frames: 0,
                misses: 0,
                worst: 0.0,
            });
        }
        None => c
            .failures
            .push(format!("no track's row is under the pointer at {y}")),
    }
}

/// Alt+wheel over a track part way down the scrolled Multitrack: the rows grow and shrink with
/// that track's row held under the pointer every frame, and Alt let go mid-notch.
fn track_zoom_steps() -> Vec<Step> {
    vec![
        Step::PointerTo("the lanes, 60 % down the rows", |app, _| {
            let (view, _) = app.track_view?;
            Some(egui::pos2(
                view.left() + crate::arrange::HEADER_WIDTH + 300.0,
                view.top() + view.height() * 0.6,
            ))
        }),
        Step::Send("the wheel, three notches down", |_| {
            notch(-3.0, Modifiers::NONE)
        }),
        Step::Wait("the rows scrolled down", |app, ctx, c| {
            notch_done(app, ctx, c)?;
            match rows_offset(app) {
                Some(o) if o > 60.0 => Ok(()),
                o => Err(format!("the rows' offset is {o:?}")),
            }
        }),
        Step::Do("the track under the pointer noted", note_zoomed_row),
        Step::Do("Alt held", |app| {
            if let Some(c) = &mut app.edit_check {
                c.modifiers = Modifiers::ALT;
            }
        }),
        Step::Send("Alt+wheel up", |_| notch(1.0, Modifiers::ALT)),
        Step::Wait("the notch done", notch_done),
        Step::Send("Alt+wheel up again", |_| notch(1.0, Modifiers::ALT)),
        Step::Wait("the notch done", notch_done),
        Step::Send("Alt+wheel up a third time", |_| notch(1.0, Modifiers::ALT)),
        Step::Wait("the notch done", notch_done),
        Step::Expect("the rows grew", |app, _, c| {
            let (Some(now), Some(w)) = (zoomed_height(app, c), c.zoom.as_ref()) else {
                return Err("no track watched".into());
            };
            if now < w.height * 1.3 {
                return Err(format!("{now} points tall, from {}", w.height));
            }
            println!("grown from {} to {now:.1} points", w.height);
            Ok(())
        }),
        Step::Shot,
        Step::Send("Alt+wheel down", |_| notch(-1.0, Modifiers::ALT)),
        Step::Wait("the notch done", notch_done),
        Step::Send("Alt+wheel down again", |_| notch(-1.0, Modifiers::ALT)),
        Step::Wait("the notch done", notch_done),
        Step::Send("Alt+wheel down a third time", |_| {
            notch(-1.0, Modifiers::ALT)
        }),
        Step::Wait("the notch done", notch_done),
        Step::Expect("the rows shrank back", |app, _, c| {
            let (Some(now), Some(w)) = (zoomed_height(app, c), c.zoom.as_ref()) else {
                return Err("no track watched".into());
            };
            if (now - w.height).abs() > w.height * 0.1 {
                return Err(format!("{now} points tall, from {}", w.height));
            }
            println!("shrunk back to {now:.1} points");
            Ok(())
        }),
        Step::Send("Alt+wheel up, to let go of Alt partway", |_| {
            notch(1.0, Modifiers::ALT)
        }),
        Step::Do("Alt let go a frame into the notch", |app| {
            let height = app
                .edit_check
                .as_ref()
                .and_then(|c| zoomed_height(app, c))
                .unwrap_or(0.0);
            if let Some(c) = &mut app.edit_check {
                c.modifiers = Modifiers::NONE;
                c.released_height = height;
            }
        }),
        Step::Wait("the notch done", notch_done),
        Step::Expect(
            "the rest of the notch grew the rows, never scrolling them",
            |app, _, c| {
                let now = zoomed_height(app, c).unwrap_or(0.0);
                if now <= c.released_height + 1.0 {
                    return Err(format!(
                        "{now} points tall, {} as Alt was let go",
                        c.released_height
                    ));
                }
                println!(
                    "grown from {:.1} to {now:.1} points after Alt was let go",
                    c.released_height
                );
                Ok(())
            },
        ),
        Step::Expect("every frame kept the row under the pointer", |_, _, c| {
            let Some(w) = c.zoom.as_ref() else {
                return Err("no track watched".into());
            };
            println!(
                "{} frames watched, the row off by at most {:.2} points",
                w.frames, w.worst
            );
            match w.misses {
                0 => Ok(()),
                n => Err(format!("{n} frames moved it a point or more")),
            }
        }),
    ]
}

/// Guitar L's clip as drawn last frame.
fn guitar_clip(app: &DawApp, c: &Check) -> Option<egui::Rect> {
    let clip = c.clip?;
    Some(app.clip_rects.iter().find(|(id, _)| *id == clip)?.1)
}

/// Low in Guitar L's clip, `along` its width: under the gain line, clear of the fade handles
/// in the title bar and of the edges.
fn inside_guitar(app: &DawApp, c: &Check, along: f32) -> Option<egui::Pos2> {
    let rect = guitar_clip(app, c)?;
    Some(egui::pos2(
        rect.left() + rect.width() * along,
        rect.bottom() - 8.0,
    ))
}

/// Where a click at `x` puts the playhead, worked out from the ruler (which starts where the
/// lanes do) and the view, not through the Multitrack's own helper: the sample under `x`,
/// snapped as the app's Snap says.
fn playhead_for(app: &DawApp, x: f32) -> Option<u64> {
    let ruler = app.ruler_rect?;
    let under = app.view_start + f64::from(x - ruler.left()) * app.samples_per_pixel;
    Some(crate::arrange::snap(app, under) as u64)
}

/// The playhead is where the last click put it, by [`playhead_for`].
fn playhead_on_click(app: &DawApp, c: &Check) -> Result<(), String> {
    let at = c.clicked.ok_or("no click")?;
    let want = playhead_for(app, at.x).ok_or("no ruler drawn")?;
    let now = app.session.position();
    if now != want {
        return Err(format!(
            "the playhead is at {now}, the click at x = {} is sample {want}",
            at.x
        ));
    }
    Ok(())
}

/// Held long enough for a locate to reach the engine and come back as the position.
fn settled(c: &Check) -> Result<(), String> {
    if c.step_started.elapsed() < Duration::from_millis(300) {
        return Err("waiting".into());
    }
    Ok(())
}

/// The playhead where it was as the last click or press went in, and every clip as it was.
fn playhead_stayed(app: &DawApp, c: &Check) -> Result<(), String> {
    let now = app.session.position();
    if now != c.position_before {
        return Err(format!(
            "the playhead moved from {} to {now}",
            c.position_before
        ));
    }
    unchanged(app, &c.held)
}

/// Clicks on clips place the playhead (snapped or not), and a drag, a fade handle, an edge or
/// a right-click don't (25.2).
fn playhead_steps() -> Vec<Step> {
    vec![
        Step::Wait("the playhead at the start", |app, _, _| {
            match app.session.position() {
                0 => Ok(()),
                at => Err(format!("at {at}")),
            }
        }),
        Step::ClickAt("inside Guitar L's clip, Snap off", |app, c| {
            inside_guitar(app, c, 0.37)
        }),
        Step::Wait(
            "the playhead on the sample clicked, the clip and Guitar L selected, the time \
             selection gone",
            |app, _, c| {
                playhead_on_click(app, c)?;
                let clip = c.clip.ok_or("no clip")?;
                if app.selected_clip_ids() != [clip] {
                    return Err(format!("selected clips {:?}", app.selected_clip_ids()));
                }
                if app.selection != Some(Selection::Track(c.track)) {
                    return Err(format!("the selection is {:?}", app.selection));
                }
                if app.time_selection.is_some() {
                    return Err("the time selection is still there".into());
                }
                unchanged(app, &c.held)?;
                no_new_undo_steps(app, c)
            },
        ),
        Step::Do("Snap on", |app| app.settings.snap = true),
        Step::ClickAt("further along the clip, Snap on", |app, c| {
            inside_guitar(app, c, 0.61)
        }),
        Step::Wait(
            "Snap on: the playhead on the 1/16 line nearest the click, the clip still selected",
            |app, _, c| {
                playhead_on_click(app, c)?;
                let at = app.session.position() as f64;
                if crate::arrange::snap(app, at) != at {
                    return Err(format!("{at} is no grid line"));
                }
                let clip = c.clip.ok_or("no clip")?;
                if app.selected_clip_ids() != [clip] {
                    return Err(format!("selected clips {:?}", app.selected_clip_ids()));
                }
                Ok(())
            },
        ),
        Step::Shot,
        Step::ClickAt("Keys' MIDI clip", |app, c| {
            let (_, midi) = c.midi?;
            let rect = app.clip_rects.iter().find(|(id, _)| *id == midi)?.1;
            Some(egui::pos2(rect.center().x, rect.bottom() - 8.0))
        }),
        Step::Wait(
            "a MIDI clip's click: the playhead there (snapped), the clip and Keys selected",
            |app, _, c| {
                playhead_on_click(app, c)?;
                let (keys, midi) = c.midi.ok_or("no MIDI clip")?;
                if app.selected_clip_ids() != [midi] {
                    return Err(format!("selected clips {:?}", app.selected_clip_ids()));
                }
                if app.selection != Some(Selection::Track(keys)) {
                    return Err(format!("the selection is {:?}", app.selection));
                }
                Ok(())
            },
        ),
        // A drag of Guitar L's clip: it moves, the playhead doesn't.
        Step::PointerDown("Guitar L's clip, to drag it", |app, c| {
            inside_guitar(app, c, 0.25)
        }),
        Step::PointerTo("20 points right", |_, c| {
            c.grab.map(|g| g + egui::vec2(20.0, 0.0))
        }),
        Step::PointerTo("40 points right", |_, c| {
            c.grab.map(|g| g + egui::vec2(40.0, 0.0))
        }),
        Step::PointerTo("60 points right", |_, c| {
            c.grab.map(|g| g + egui::vec2(60.0, 0.0))
        }),
        Step::PointerUp,
        Step::Wait("a moment for any locate", |_, _, c| settled(c)),
        Step::Expect(
            "dragged 60 points: the clip moved later, the playhead stayed",
            |app, _, c| {
                let clip = c.clip.ok_or("no clip")?;
                let then = c.held.iter().find(|k| k.id == clip).ok_or("no clip")?;
                let now = app.session.project.find_clip(clip).ok_or("gone")?.1;
                if now.start <= then.start {
                    return Err(format!(
                        "it starts at {}, {} before the drag",
                        now.start, then.start
                    ));
                }
                if app.group_drag.is_some() {
                    return Err("the drag is still under way".into());
                }
                let now = app.session.position();
                if now != c.position_before {
                    return Err(format!(
                        "the playhead moved from {} to {now}",
                        c.position_before
                    ));
                }
                Ok(())
            },
        ),
        // Its fade-out handle: hovered, then clicked.
        Step::PointerTo("Guitar L's fade-out handle", fade_out_handle),
        Step::Wait("a moment on it", |_, _, c| settled(c)),
        Step::ClickAt("the fade-out handle", fade_out_handle),
        Step::Wait("a moment for any locate", |_, _, c| settled(c)),
        Step::Expect(
            "a click on the fade-out handle: the playhead and the clips as they were",
            |app, _, c| playhead_stayed(app, c),
        ),
        // Its end: hovered until lit, then clicked.
        Step::PointerTo("Guitar L's clip's end", |app, c| {
            let rect = guitar_clip(app, c)?;
            Some(egui::pos2(rect.right() - 2.0, rect.bottom() - 8.0))
        }),
        Step::Wait("its end is lit", |app, _, c| {
            let want = crate::arrange::LitEdge {
                clip: c.clip.ok_or("no clip")?,
                left: false,
                hidden: false,
            };
            if app.edge_lit != Some(want) {
                return Err(format!("the lit edge is {:?}", app.edge_lit));
            }
            Ok(())
        }),
        Step::ClickAt("the clip's end", |_, c| c.pointer),
        Step::Wait("a moment for any locate", |_, _, c| settled(c)),
        Step::Expect(
            "a click on the end: the playhead and the clips as they were",
            |app, _, c| playhead_stayed(app, c),
        ),
        Step::RightClickAt("Guitar L's clip", |app, c| inside_guitar(app, c, 0.5)),
        Step::Wait("a moment for any locate", |_, _, c| settled(c)),
        Step::Expect(
            "a right-click on the clip: the playhead and the clips as they were",
            |app, _, c| playhead_stayed(app, c),
        ),
        // Double-clicks (26.2), Snap off so the first click puts the playhead (and its handle)
        // right under the pointer for the second: the menu closed first.
        Step::Press(Key::Escape),
        Step::Do("Snap off", |app| app.settings.snap = false),
        Step::DoubleClickAt("Keys' MIDI clip", |app, c| {
            let (_, midi) = c.midi?;
            let rect = app.clip_rects.iter().find(|(id, _)| *id == midi)?.1;
            Some(egui::pos2(
                rect.left() + rect.width() * 0.3,
                rect.bottom() - 8.0,
            ))
        }),
        // Expected, not waited for, so a failure here still lets the next one run.
        Step::Wait("a moment for the locate", |_, _, c| settled(c)),
        Step::Expect(
            "a MIDI clip's double-click: the piano roll open on it, the playhead at the click, \
             the clip selected",
            |app, _, c| {
                let (_, midi) = c.midi.ok_or("no MIDI clip")?;
                let open = app.piano_roll.as_ref().map(|p| p.clip);
                if open != Some(midi) {
                    return Err(format!("the piano roll is open on {open:?}, not {midi}"));
                }
                playhead_at_double_click(app, c)?;
                if app.selected_clip_ids() != [midi] {
                    return Err(format!("selected clips {:?}", app.selected_clip_ids()));
                }
                unchanged(app, &c.held)
            },
        ),
        Step::DoubleClickAt("Guitar L's clip", |app, c| inside_guitar(app, c, 0.45)),
        // Expected, not waited for, so a failure here still lets the next one run.
        Step::Wait("a moment for the locate", |_, _, c| settled(c)),
        Step::Expect(
            "an audio clip's double-click: the Waveform view open on its file, the playhead at \
             the click",
            |app, _, c| {
                let clip = c.clip.ok_or("no clip")?;
                if app.view_mode != daw_core::api::ViewMode::Waveform {
                    return Err(format!("the view is {:?}", app.view_mode));
                }
                let doc = app.editor.current.ok_or("no file open")?;
                let source = app.session.document(doc).and_then(|d| d.source);
                if source != Some((c.track, clip)) {
                    return Err(format!("the open file is from {source:?}"));
                }
                playhead_at_double_click(app, c)?;
                unchanged(app, &c.held)
            },
        ),
    ]
}

/// The playhead where the last `DoubleClickAt` clicked (worked out as it went in: the view
/// changes when it opens the Waveform view).
fn playhead_at_double_click(app: &DawApp, c: &Check) -> Result<(), String> {
    let want = c.expected.ok_or("no click")?;
    let now = app.session.position();
    if now != want {
        return Err(format!(
            "the playhead is at {now}, the click at sample {want}"
        ));
    }
    Ok(())
}

/// Guitar L's fade-out handle: on the title bar's lower edge at the fade-out's inner end.
fn fade_out_handle(app: &DawApp, c: &Check) -> Option<egui::Pos2> {
    let rect = guitar_clip(app, c)?;
    let clip = app.session.project.find_clip(c.clip?)?.1;
    let x = rect.right() - (clip.fade_out.length as f64 / app.samples_per_pixel) as f32;
    // The waveform starts 14 points down; the handle's centre is 4 below that.
    Some(egui::pos2(x, rect.top() + 18.0))
}

/// No undo step since the check began.
fn no_new_undo_steps(app: &DawApp, c: &Check) -> Result<(), String> {
    let made = app.session.history().0.len().saturating_sub(c.undo_before);
    if made == 0 {
        Ok(())
    } else {
        Err(format!("{made} undo step(s) since the start"))
    }
}

/// The playhead drag check's layout (26.3): a 1/8 grid with Snap on (the defaults), zoomed in
/// to 8 samples a point (a 1/8's lines over a thousand points apart, so the grid line nearest
/// a point by an edge is often past it), the view 22,050 samples in and the playhead 400
/// points into the lanes.
fn playhead_drag_layout(app: &mut DawApp) {
    app.session.project.grid = daw_core::grid::Grid::Fixed(daw_core::grid::Division::note(8));
    app.settings.snap = true;
    app.samples_per_pixel = 8.0;
    app.view_start = 22_050.0;
    let located = app.session.locate(22_050 + 400 * 8);
    app.report(located);
    app.selection = None;
}

/// The piano roll check's layout (26.4): the piano roll open on Keys' clip (as it opens: 8
/// ticks a point, the song's grid, the editor's Snap on), its first note selected, the
/// playhead at the start (off the view's left).
fn piano_click_layout(app: &mut DawApp, keys: TrackId) -> Option<(TrackId, ClipId)> {
    let clip = app.session.project.track(keys)?.midi_clips.first()?;
    let (id, first) = (clip.id, clip.notes.first()?.id);
    crate::piano_roll::open(app, id).ok()?;
    app.piano_roll.as_mut()?.selected = [first].into();
    let located = app.session.locate(0);
    app.report(located);
    Some((keys, id))
}

/// Keys' clip as it is now.
fn keys_clip(app: &DawApp, c: &Check) -> Option<daw_core::midi_clip::MidiClip> {
    let (_, id) = c.midi?;
    Some(app.session.project.find_midi_clip(id)?.1.clone())
}

/// The piano roll's parts as drawn last frame.
fn piano_drawn(app: &DawApp) -> Option<crate::piano_roll::Drawn> {
    app.piano_roll.as_ref()?.drawn
}

/// Where a click at `x` in the piano roll puts the playhead, worked out from what was drawn
/// and the editor's snap, not through the piano roll's own helper: the song tick under `x`
/// (in the grid's width), on the grid line nearest it with Snap on, in samples through the
/// tempo map.
fn piano_playhead_for(app: &DawApp, c: &Check, x: f32) -> Option<u64> {
    let drawn = piano_drawn(app)?;
    let clip = keys_clip(app, c)?;
    let x = x.clamp(drawn.grid.left(), drawn.grid.right() - 1.0);
    let song = (clip.start as f64 + drawn.tick(x)).max(0.0).round() as u64;
    let map = &app.tempo_map;
    let tick = match app.piano_roll.as_ref()?.division() {
        Some(d) => map.snap_tick(song, d, daw_core::grid::Rounding::Nearest),
        None => song,
    };
    Some(map.sample_at_tick(tick as f64).round().max(0.0) as u64)
}

/// The playhead where the last click in the piano roll put it, by [`piano_playhead_for`].
fn piano_playhead_on_click(app: &DawApp, c: &Check) -> Result<(), String> {
    let at = c.clicked.ok_or("no click")?;
    let want = piano_playhead_for(app, c, at.x).ok_or("no piano roll drawn")?;
    let now = app.session.position();
    if now != want {
        return Err(format!(
            "the playhead is at {now}, the click at x = {} is sample {want}",
            at.x
        ));
    }
    Ok(())
}

/// A note's rectangle in the piano roll as drawn last frame (its row's full height).
fn note_rect(d: &crate::piano_roll::Drawn, n: &Note) -> egui::Rect {
    let start = d.point(n.tick as f64, n.pitch);
    let end = d.point((n.tick + n.length) as f64, n.pitch);
    egui::Rect::from_min_max(
        egui::pos2(start.x, start.y - 6.0),
        egui::pos2(end.x.max(start.x + 2.0), end.y + 6.0),
    )
}

/// Empty grid `along` the grid's width (moved on past the playhead's line if it is there): the
/// middle of the row nearest the grid's middle with no note within 12 points of it.
fn empty_grid(app: &DawApp, c: &Check, along: f32) -> Option<egui::Pos2> {
    let d = piano_drawn(app)?;
    let clip = keys_clip(app, c)?;
    let mut x = d.grid.left() + d.grid.width() * along;
    while d.line_x.is_some_and(|l| (l - x).abs() < 10.0) {
        x += 20.0;
    }
    let middle = d.grid.center().y;
    let mut rows: Vec<u8> = (0..=127u8)
        .filter(|&p| {
            let y = d.point(0.0, p).y;
            y - 6.0 >= d.grid.top() && y + 6.0 <= d.grid.bottom()
        })
        .collect();
    rows.sort_by(|a, b| {
        let off = |p: u8| (d.point(0.0, p).y - middle).abs();
        off(*a).total_cmp(&off(*b))
    });
    rows.into_iter().find_map(|p| {
        let at = egui::pos2(x, d.point(0.0, p).y);
        let near = egui::Rect::from_center_size(at, egui::vec2(24.0, 4.0));
        let free = clip
            .notes
            .iter()
            .all(|n| !note_rect(&d, n).intersects(near));
        free.then_some(at)
    })
}

/// The pitch of the row at `y` in the piano roll as drawn.
fn piano_pitch(d: &crate::piano_roll::Drawn, y: f32) -> u8 {
    (d.top_pitch - (y - d.grid.top()) / 12.0)
        .ceil()
        .clamp(0.0, 127.0) as u8
}

/// The picked note's middle, as drawn last frame.
fn picked_note(app: &DawApp, c: &Check) -> Option<egui::Pos2> {
    let d = piano_drawn(app)?;
    let id = c.piano_note?;
    let clip = keys_clip(app, c)?;
    Some(note_rect(&d, clip.notes.iter().find(|n| n.id == id)?).center())
}

/// Keys' notes as they are now.
fn keys_notes(app: &DawApp) -> Vec<Note> {
    app.edit_check
        .as_ref()
        .and_then(|c| keys_clip(app, c))
        .map(|k| k.notes)
        .unwrap_or_default()
}

/// A note to click and drag: the first whose middle is on screen, at least 40 points wide, and
/// not under the playhead's line; the notes noted.
fn pick_note(app: &mut DawApp) {
    let picked = app.edit_check.as_ref().and_then(|c| {
        let d = piano_drawn(app)?;
        let clip = keys_clip(app, c)?;
        clip.notes.iter().find_map(|n| {
            let r = note_rect(&d, n);
            let on = d.grid.shrink(4.0).contains(r.center()) && r.width() >= 40.0;
            let clear = d.line_x.is_none_or(|l| (l - r.center().x).abs() > 10.0);
            (on && clear).then_some(n.id)
        })
    });
    let notes = keys_notes(app);
    if let Some(c) = &mut app.edit_check {
        c.piano_note = picked;
        c.notes_then = notes;
        if picked.is_none() {
            c.failures.push("no note to click on screen".into());
        }
    }
}

/// Keys' notes noted as they are now.
fn note_notes(app: &mut DawApp) {
    let notes = keys_notes(app);
    if let Some(c) = &mut app.edit_check {
        c.notes_then = notes;
    }
}

/// Keys' notes as last noted.
fn notes_unchanged(app: &DawApp, c: &Check) -> Result<(), String> {
    let now = keys_clip(app, c).ok_or("no clip")?.notes;
    if now != c.notes_then {
        return Err(format!(
            "the notes changed ({} now, {} before)",
            now.len(),
            c.notes_then.len()
        ));
    }
    Ok(())
}

/// The note selection in the piano roll.
fn piano_selected(app: &DawApp) -> Vec<NoteId> {
    app.piano_roll
        .as_ref()
        .map(|p| p.selected.iter().copied().collect())
        .unwrap_or_default()
}

/// Away from the playhead's line as drawn, `along` the velocity lane, in its middle.
fn velocity_lane(app: &DawApp, _: &Check) -> Option<egui::Pos2> {
    let d = piano_drawn(app)?;
    let mut x = d.lane.left() + d.lane.width() * 0.45;
    while d.line_x.is_some_and(|l| (l - x).abs() < 10.0) {
        x += 20.0;
    }
    Some(egui::pos2(x, d.lane.center().y))
}

/// Clicks in the piano roll's note grid place the playhead (Snap on and off, empty grid and a
/// note); a double-click still draws a note; a note's drag, a key and the velocity lane leave
/// the playhead where it was (26.4).
/// The pages a check run asked the manual for, as `page#anchor`.
fn asked(app: &DawApp) -> Vec<String> {
    app.manual_requests
        .iter()
        .map(|t| format!("{}#{}", t.page, t.anchor.as_deref().unwrap_or("")))
        .collect()
}

/// `place`'s page as F1's table names it, as `page#anchor`.
fn page_of(place: crate::help::Place) -> String {
    let (page, anchor) = place.page();
    format!("{page}#{}", anchor.unwrap_or(""))
}

/// Help > Manual, then F1 over the piano roll and in Settings > Audio (27.3).
fn help_steps() -> Vec<Step> {
    use crate::help::Place;
    use crate::settings_window::Section;
    vec![
        Step::Click(crate::dialogs::help_menu_id()),
        Step::Wait("the Help menu opened, Manual on it", |_, ctx, _| {
            if !crate::dialogs::help_menu_open(ctx) {
                return Err("no menu".into());
            }
            same(
                drawn(ctx, crate::dialogs::manual_item_id()).as_deref(),
                Some("Manual"),
            )
        }),
        // Half a second for the menu to fade in before the shot.
        Step::Wait("the menu stayed open", |_, ctx, c| {
            if !crate::dialogs::help_menu_open(ctx) {
                return Err("it closed".into());
            }
            if c.step_started.elapsed() < Duration::from_millis(500) {
                return Err("not half a second yet".into());
            }
            Ok(())
        }),
        Step::Shot,
        Step::Click(crate::dialogs::manual_item_id()),
        Step::Wait("Manual asked for the manual's first page", |app, _, _| {
            let asked = asked(app);
            if asked == ["index#"] {
                Ok(())
            } else {
                Err(format!("asked for {asked:?}"))
            }
        }),
        Step::Do("Keys' clip opened in the piano roll", |app| {
            let clip = app
                .session
                .project
                .tracks
                .iter()
                .flat_map(|t| &t.midi_clips)
                .map(|c| c.id)
                .next();
            if let Some(clip) = clip {
                let _ = crate::piano_roll::open(app, clip);
            }
        }),
        Step::Wait("the piano roll drawn", |app, _, _| {
            app.help
                .rect_of(Place::PianoRoll)
                .map(|_| ())
                .ok_or_else(|| "not drawn".to_string())
        }),
        Step::PointerTo("the middle of the piano roll", |app, _| {
            app.help.rect_of(Place::PianoRoll).map(|r| r.center())
        }),
        Step::Press(egui::Key::F1),
        Step::Wait("F1 over the piano roll asked for its page", |app, _, _| {
            let asked = asked(app);
            let want = ["index#".to_string(), page_of(Place::PianoRoll)];
            if asked == want {
                Ok(())
            } else {
                Err(format!("asked for {asked:?}, not {want:?}"))
            }
        }),
        Step::Do("Options > Settings… on Audio", |app| {
            app.open_settings(Section::Audio)
        }),
        Step::Wait("the settings window drawn", |app, _, _| {
            app.help
                .rect_of(Place::Settings(Section::Audio))
                .map(|_| ())
                .ok_or_else(|| "not drawn".to_string())
        }),
        Step::PointerTo("the settings window", |app, _| {
            app.help
                .rect_of(Place::Settings(Section::Audio))
                .map(|r| r.center())
        }),
        Step::Press(egui::Key::F1),
        Step::Wait(
            "F1 in Settings > Audio asked for its section",
            |app, _, _| {
                let asked = asked(app);
                let want = [
                    "index#".to_string(),
                    page_of(Place::PianoRoll),
                    page_of(Place::Settings(Section::Audio)),
                ];
                if asked == want {
                    Ok(())
                } else {
                    Err(format!("asked for {asked:?}, not {want:?}"))
                }
            },
        ),
        Step::Expect(
            "the pages are the piano roll's and settings#audio, and no browser started (a \
             check run notes each request instead)",
            |app, _, _| {
                if page_of(Place::PianoRoll) != "piano-roll#"
                    || page_of(Place::Settings(Section::Audio)) != "settings#audio"
                {
                    return Err("F1's table changed: update this check".into());
                }
                if !app.is_check_run() {
                    return Err("not a check run: a browser may have started".into());
                }
                Ok(())
            },
        ),
    ]
}

fn piano_click_steps() -> Vec<Step> {
    vec![
        Step::Wait(
            "the piano roll drawn on Keys' clip, the playhead at the start",
            |app, _, c| {
                let (_, midi) = c.midi.ok_or("no MIDI clip")?;
                let open = app.piano_roll.as_ref().map(|p| p.clip);
                if open != Some(midi) {
                    return Err(format!("the piano roll is open on {open:?}"));
                }
                piano_drawn(app).ok_or("not drawn yet")?;
                match app.session.position() {
                    0 => Ok(()),
                    at => Err(format!("the playhead at {at}")),
                }
            },
        ),
        Step::Do("Keys' notes noted", note_notes),
        Step::ClickAt("empty grid, Snap on", |app, c| empty_grid(app, c, 0.35)),
        Step::Wait(
            "Snap on: the playhead on the grid line nearest the click, the selection cleared",
            |app, _, c| {
                piano_playhead_on_click(app, c)?;
                if app.piano_roll.as_ref().and_then(|p| p.division()).is_none() {
                    return Err("the editor's Snap is off".into());
                }
                if !piano_selected(app).is_empty() {
                    return Err(format!("notes selected: {:?}", piano_selected(app)));
                }
                notes_unchanged(app, c)?;
                no_new_undo_steps(app, c)
            },
        ),
        Step::Do("the piano roll's Snap off", |app| {
            if let Some(p) = app.piano_roll.as_mut() {
                p.set_snap(false);
            }
        }),
        Step::ClickAt("empty grid further along, Snap off", |app, c| {
            empty_grid(app, c, 0.55)
        }),
        Step::Wait("Snap off: the playhead on the tick clicked", |app, _, c| {
            piano_playhead_on_click(app, c)?;
            if app.piano_roll.as_ref().and_then(|p| p.division()).is_some() {
                return Err("the editor's Snap is on".into());
            }
            notes_unchanged(app, c)
        }),
        Step::Do("a note picked", pick_note),
        Step::ClickAt("the middle of a note", picked_note),
        Step::Wait(
            "a note's click: the playhead there, the note selected",
            |app, _, c| {
                piano_playhead_on_click(app, c)?;
                let picked = c.piano_note.ok_or("no note picked")?;
                if piano_selected(app) != [picked] {
                    return Err(format!(
                        "notes selected: {:?}, not {picked}",
                        piano_selected(app)
                    ));
                }
                notes_unchanged(app, c)
            },
        ),
        // The same note dragged from where it was clicked: the playhead's line is under the
        // pointer there (Snap off), hovered first as a hand would.
        Step::PointerTo("the clicked note again", picked_note),
        Step::Wait("a moment on it", |_, _, c| settled(c)),
        Step::Do("Keys' notes noted", note_notes),
        Step::PointerDown("the note, to drag it", picked_note),
        Step::PointerTo("20 points right", |_, c| {
            c.grab.map(|g| g + egui::vec2(20.0, 0.0))
        }),
        Step::PointerTo("40 points right", |_, c| {
            c.grab.map(|g| g + egui::vec2(40.0, 0.0))
        }),
        Step::PointerTo("60 points right", |_, c| {
            c.grab.map(|g| g + egui::vec2(60.0, 0.0))
        }),
        Step::PointerUp,
        Step::Wait("a moment for any locate", |_, _, c| settled(c)),
        Step::Expect(
            "the note dragged 60 points: it moved later, the playhead stayed",
            |app, _, c| {
                let id = c.piano_note.ok_or("no note picked")?;
                let then = c.notes_then.iter().find(|n| n.id == id).ok_or("no note")?;
                let clip = keys_clip(app, c).ok_or("no clip")?;
                let now = clip.notes.iter().find(|n| n.id == id).ok_or("gone")?;
                if now.tick <= then.tick {
                    return Err(format!(
                        "it starts at tick {}, {} before the drag",
                        now.tick, then.tick
                    ));
                }
                let at = app.session.position();
                if at != c.position_before {
                    return Err(format!(
                        "the playhead moved from {} to {at}",
                        c.position_before
                    ));
                }
                Ok(())
            },
        ),
        Step::Do("Keys' notes noted", note_notes),
        Step::DoubleClickAt("empty grid", |app, c| empty_grid(app, c, 0.75)),
        // Expected, not waited for, so a failure here still lets the next ones run.
        Step::Wait("a moment for the locate", |_, _, c| settled(c)),
        Step::Expect(
            "a double-click on empty grid: a note drawn there, the playhead at the click",
            |app, _, c| {
                playhead_at_double_click(app, c)?;
                let at = c.clicked.ok_or("no click")?;
                let d = piano_drawn(app).ok_or("not drawn")?;
                let clip = keys_clip(app, c).ok_or("no clip")?;
                let new: Vec<&Note> = clip
                    .notes
                    .iter()
                    .filter(|n| !c.notes_then.iter().any(|t| t.id == n.id))
                    .collect();
                let [note] = new.as_slice() else {
                    return Err(format!("{} new notes", new.len()));
                };
                // Snap off: the note starts on the tick under the pointer, in its row.
                let tick = d.tick(at.x).max(0.0).round() as u64;
                let pitch = piano_pitch(&d, at.y);
                if note.tick != tick || note.pitch != pitch {
                    return Err(format!(
                        "the note is at tick {} pitch {}, the click at tick {tick} pitch {pitch}",
                        note.tick, note.pitch
                    ));
                }
                if piano_selected(app) != [note.id] {
                    return Err(format!("notes selected: {:?}", piano_selected(app)));
                }
                Ok(())
            },
        ),
        Step::Shot,
        Step::Do("Keys' notes noted", note_notes),
        Step::ClickAt("a key on the keyboard", |app, _| {
            let d = piano_drawn(app)?;
            Some(egui::pos2(d.keys.left() + 10.0, d.keys.center().y))
        }),
        Step::Wait("a moment for any locate", |_, _, c| settled(c)),
        Step::Expect(
            "a click on a key: the playhead and the notes as they were",
            |app, _, c| {
                playhead_stayed(app, c)?;
                notes_unchanged(app, c)
            },
        ),
        Step::ClickAt("the velocity lane", velocity_lane),
        Step::Wait("a moment for any locate", |_, _, c| settled(c)),
        Step::Expect(
            "a click on the velocity lane: the playhead and the notes as they were",
            |app, _, c| {
                playhead_stayed(app, c)?;
                notes_unchanged(app, c)
            },
        ),
    ]
}

/// Frames a playhead drag's scrolling isn't judged after the pointer jumps (the frame that
/// takes the jump, and egui's pointer settling after a `PointerGone`).
const CALM: u32 = 2;

/// A playhead drag watched every frame from the press to the release (26.3).
struct DragWatch {
    /// The view's start last frame.
    view: f64,
    /// Frames left before the scrolling is judged again.
    calm: u32,
    /// Frames watched; those egui wasn't dragging the handle on, the playhead wasn't drawn in
    /// the lanes, and the view didn't scroll the pointer's way.
    frames: u32,
    lost: u32,
    hidden: u32,
    stuck: u32,
}

/// The playhead's handle as drawn last frame.
fn playhead_handle(app: &DawApp, _: &Check) -> Option<egui::Pos2> {
    app.playhead_grab.map(|(p, _)| p)
}

/// The view's start noted, to compare with later.
fn note_view(app: &mut DawApp) {
    let view = app.view_start;
    if let Some(c) = &mut app.edit_check {
        c.noted_view = view;
    }
}

/// The current drag watched every frame from now until it is let go.
fn start_drag_watch(app: &mut DawApp) {
    let view = app.view_start;
    if let Some(c) = &mut app.edit_check {
        c.drag = Some(DragWatch {
            view,
            calm: CALM,
            frames: 0,
            lost: 0,
            hidden: 0,
            stuck: 0,
        });
    }
}

/// The pointer gone (out of the window, as Windows' mouse-leave message makes it), the button
/// still down.
fn pointer_gone(app: &mut DawApp) {
    if let Some(c) = &mut app.edit_check {
        c.gone = c.pointer.take();
        c.gone_sent = false;
        if let Some(w) = &mut c.drag {
            w.calm = CALM;
        }
    }
}

/// Far past the lanes' right edge (`right`) or left edge, out of the window, at the press's
/// height.
fn far_past(app: &DawApp, c: &Check, right: bool) -> Option<egui::Pos2> {
    let ruler = app.ruler_rect?;
    let y = c.grab?.y;
    Some(if right {
        egui::pos2(ruler.right() + 600.0, y)
    } else {
        egui::pos2(ruler.left() - 600.0, y)
    })
}

/// `ms` milliseconds into the step.
fn held(c: &Check, ms: u64) -> Result<(), String> {
    if c.step_started.elapsed() < Duration::from_millis(ms) {
        return Err("holding".into());
    }
    Ok(())
}

/// The view scrolled later (`right`) or back since it was noted.
fn scrolled(app: &DawApp, c: &Check, right: bool) -> Result<(), String> {
    let (then, now) = (c.noted_view, app.view_start);
    let points = (now - then) / app.samples_per_pixel;
    let went = if right {
        points > 100.0
    } else {
        points < -100.0
    };
    if !went {
        return Err(format!(
            "the view's start went from {then:.0} to {now:.0} ({points:+.0} points)"
        ));
    }
    println!("the view scrolled {points:+.0} points");
    Ok(())
}

/// No drag of the playhead under way.
fn drag_over(app: &DawApp, ctx: &egui::Context) -> Result<(), String> {
    if app.playhead_handle.is_some() && ctx.dragged_id() == app.playhead_handle {
        return Err("the playhead is still being dragged".into());
    }
    Ok(())
}

/// Where the session's playhead is in the lanes now, in points.
fn playhead_x(app: &DawApp) -> Option<f32> {
    let ruler = app.ruler_rect?;
    let at = app.session.position() as f64;
    Some(ruler.left() + ((at - app.view_start) / app.samples_per_pixel) as f32)
}

/// Every frame of a watched drag (26.3): egui is still dragging the playhead's handle, the
/// playhead is drawn in the lanes, and the view scrolls the pointer's way while it is past an
/// edge (back to the start at the least).
fn watch_drag(app: &mut DawApp, ctx: &egui::Context) {
    let Some(c) = &app.edit_check else {
        return;
    };
    if c.drag.is_none() {
        return;
    }
    let Some(ruler) = app.ruler_rect else {
        return;
    };
    let dragged = app.playhead_handle.is_some() && ctx.dragged_id() == app.playhead_handle;
    let drawn = app.playhead_grab.map(|(p, _)| p.x);
    let view = app.view_start;
    let engine = playhead_x(app).unwrap_or(f32::NAN);
    let pointer = c.pointer.or(c.gone).map(|p| p.x);
    let step = c.describe();
    let c = app.edit_check.as_mut().expect("checked");
    let frame = c.frame;
    let Some(w) = &mut c.drag else {
        return;
    };
    w.frames += 1;
    let seen = || {
        format!(
            "{step}, frame {frame}: the pointer at {}, the playhead drawn at {}, the session's \
             at {engine:.0}, the lanes {:.0}..{:.0}, the view's start {view:.0}",
            pointer.map_or("(gone)".to_string(), |x| format!("{x:.0}")),
            drawn.map_or("(nowhere)".to_string(), |x| format!("{x:.0}")),
            ruler.left(),
            ruler.right()
        )
    };
    let mut lines = Vec::new();
    if !dragged {
        w.lost += 1;
        if w.lost <= 3 {
            lines.push(format!("the drag was lost: {}", seen()));
        }
    }
    let shown = drawn.is_some_and(|x| x >= ruler.left() - 1.0 && x <= ruler.right());
    if !shown {
        w.hidden += 1;
        if w.hidden <= 3 {
            lines.push(format!("the playhead off screen: {}", seen()));
        }
    }
    if w.calm > 0 {
        w.calm -= 1;
    } else if let Some(x) = pointer {
        let right = x > ruler.right() && view <= w.view;
        let left = x < ruler.left() && view >= w.view && view > 0.0;
        if right || left {
            w.stuck += 1;
            if w.stuck <= 3 {
                lines.push(format!(
                    "the view didn't scroll {}: {}",
                    if right { "right" } else { "left" },
                    seen()
                ));
            }
        }
    }
    w.view = view;
    c.failures.extend(lines);
}

/// A watched drag let go: what it saw.
fn end_drag_watch(c: &mut Check) {
    let Some(w) = c.drag.take() else {
        return;
    };
    println!(
        "{} frames of the drag watched: {} lost, {} with the playhead off screen, {} not \
         scrolling the pointer's way",
        w.frames, w.lost, w.hidden, w.stuck
    );
    for (n, what) in [
        (w.lost, "the drag lost"),
        (w.hidden, "the playhead off screen"),
        (w.stuck, "the view not scrolling the pointer's way"),
    ] {
        if n > 0 {
            c.failures.push(format!("{n} frames with {what}"));
        }
    }
}

/// A fast playhead drag past either edge, zoomed in, the pointer out of the window and gone:
/// the drag holds, the view follows, the playhead stays on screen (26.3).
fn playhead_drag_steps() -> Vec<Step> {
    vec![
        Step::Wait("the playhead drawn in the lanes", |app, _, _| {
            app.playhead_grab
                .map(|_| ())
                .ok_or_else(|| "not drawn".to_string())
        }),
        Step::PointerDown("the playhead's handle, Snap on", playhead_handle),
        Step::Do("the drag watched every frame", start_drag_watch),
        Step::Do("the view's start noted", note_view),
        Step::PointerTo(
            "far past the lanes' right edge, out of the window, in one frame",
            |app, c| far_past(app, c, true),
        ),
        Step::Wait("held there half a second", |_, _, c| held(c, 500)),
        Step::Expect("the view scrolled right", |app, _, c| {
            scrolled(app, c, true)
        }),
        Step::Shot,
        Step::Do("the view's start noted", note_view),
        Step::Do(
            "the pointer gone, as Windows' mouse-leave message makes it",
            pointer_gone,
        ),
        Step::Wait("held out of the window 0.4 s", |_, _, c| held(c, 400)),
        Step::Expect(
            "the view went on scrolling right with no pointer",
            |app, _, c| scrolled(app, c, true),
        ),
        Step::PointerTo(
            "far past the lanes' left edge, out of the window, in one frame",
            |app, c| far_past(app, c, false),
        ),
        Step::Wait("the view back at the start", |app, _, _| {
            if app.view_start == 0.0 {
                Ok(())
            } else {
                Err(format!("the view starts at {:.0}", app.view_start))
            }
        }),
        Step::Wait("the playhead at the start", |app, _, _| {
            match app.session.position() {
                0 => Ok(()),
                at => Err(format!("at {at}")),
            }
        }),
        Step::PointerUp,
        Step::Wait("a moment for any locate", |_, _, c| settled(c)),
        Step::Expect(
            "let go: the drag over, the view and the playhead at the start",
            |app, ctx, _| {
                drag_over(app, ctx)?;
                if app.view_start != 0.0 || app.session.position() != 0 {
                    return Err(format!(
                        "the view starts at {:.0}, the playhead at {}",
                        app.view_start,
                        app.session.position()
                    ));
                }
                Ok(())
            },
        ),
        // Snap off: thrown past the right edge and let go out there.
        Step::Do("Snap off", |app| app.settings.snap = false),
        Step::PointerDown(
            "the playhead's handle at the start, Snap off",
            playhead_handle,
        ),
        Step::Do("the drag watched every frame", start_drag_watch),
        Step::Do("the view's start noted", note_view),
        Step::PointerTo(
            "far past the lanes' right edge, out of the window, in one frame",
            |app, c| far_past(app, c, true),
        ),
        Step::Wait("held there half a second", |_, _, c| held(c, 500)),
        Step::Expect("the view scrolled right", |app, _, c| {
            scrolled(app, c, true)
        }),
        Step::PointerUp,
        Step::Do("the view's start noted as it was let go", note_view),
        Step::Wait("a moment for the locate", |_, _, c| settled(c)),
        Step::Expect(
            "let go out of the window: the drag over, the view still, the playhead on screen \
             by the right edge",
            |app, ctx, c| {
                drag_over(app, ctx)?;
                if app.view_start != c.noted_view {
                    return Err(format!(
                        "the view moved from {:.0} to {:.0} after the release",
                        c.noted_view, app.view_start
                    ));
                }
                let ruler = app.ruler_rect.ok_or("no ruler drawn")?;
                let x = playhead_x(app).ok_or("no ruler drawn")?;
                if !(ruler.right() - 30.0..=ruler.right()).contains(&x) {
                    return Err(format!(
                        "the playhead is at x = {x:.0}, the lanes end at {:.0}",
                        ruler.right()
                    ));
                }
                Ok(())
            },
        ),
    ]
}

fn rename_steps(track: TrackId, bus: Option<TrackId>, other: Option<TrackId>) -> Vec<Step> {
    let header = value_edit::name_id(Place::Header, Some(track));
    let inspector = value_edit::name_id(Place::Inspector, None);
    let away = crate::inspector::kind_id();
    let mut steps = vec![
        Step::DoubleClick(header),
        Step::Open(header, true),
        Step::Type("Lead guitar"),
        Step::Shot,
        Step::Expect("the header's field holds what was typed", |_, ctx, c| {
            let id = value_edit::name_id(Place::Header, Some(c.track));
            same(value_edit::text(ctx, id).as_deref(), Some("Lead guitar"))
        }),
        Step::Press(Key::Enter),
        Step::Open(header, false),
        Step::Expect(
            "Enter renamed it, in the project, the header and the inspector",
            |app, ctx, c| {
                name_everywhere(app, ctx, c, "Lead guitar")?;
                undo_steps(app, c, 1, "Rename track")
            },
        ),
        // The inspector: a blank name refused, then Escape.
        Step::Click(inspector),
        Step::Open(inspector, true),
        Step::Type("   "),
        Step::Press(Key::Enter),
        Step::Expect(
            "Enter on a blank name is refused, the field kept open",
            |app, ctx, c| {
                let id = value_edit::name_id(Place::Inspector, None);
                if !value_edit::is_open(ctx, id) || !value_edit::refused(ctx, id) {
                    return Err("the field closed, or isn't marked refused".into());
                }
                same(name_of(app, c.track).as_deref(), Some("Lead guitar"))
            },
        ),
        Step::Press(Key::Escape),
        Step::Open(inspector, false),
        Step::Expect("Escape kept the old name", |app, ctx, c| {
            name_everywhere(app, ctx, c, "Lead guitar")?;
            undo_steps(app, c, 1, "Rename track")
        }),
        // The inspector again: a name kept by clicking away.
        Step::Click(inspector),
        Step::Open(inspector, true),
        Step::Type("Rhythm"),
        Step::Click(away),
        Step::Open(inspector, false),
        Step::Expect("clicking away renamed it", |app, ctx, c| {
            name_everywhere(app, ctx, c, "Rhythm")?;
            undo_steps(app, c, 2, "Rename track")
        }),
        // Escape in the header.
        Step::DoubleClick(header),
        Step::Open(header, true),
        Step::Type("Nope"),
        Step::Press(Key::Escape),
        Step::Open(header, false),
        Step::Expect("Escape in the header kept the name", |app, ctx, c| {
            name_everywhere(app, ctx, c, "Rhythm")?;
            undo_steps(app, c, 2, "Rename track")
        }),
    ];
    if let Some(bus) = bus {
        let field = value_edit::name_id(Place::Header, Some(bus));
        steps.extend([
            Step::DoubleClick(field),
            Step::Open(field, true),
            Step::Type("Room"),
            Step::Press(Key::Enter),
            Step::Open(field, false),
            Step::Expect("a bus renames the same way", |app, _, c| {
                let bus = c.bus.expect("checked");
                same(name_of(app, bus).as_deref(), Some("Room"))?;
                undo_steps(app, c, 3, "Rename track")
            }),
        ]);
    }
    if let Some(other) = other {
        // Another track selected while a name is typed in the inspector: the track it was
        // typed for keeps it, as clicking away would. (Guitar L selected again first: the
        // bus's double-click selected the bus.)
        steps.extend([
            Step::Click(header),
            Step::Click(inspector),
            Step::Open(inspector, true),
            Step::Type("Lead"),
            Step::Click(value_edit::name_id(Place::Header, Some(other))),
            Step::Open(inspector, false),
            Step::Expect(
                "selecting another track kept the name typed",
                |app, _, c| {
                    same(name_of(app, c.track).as_deref(), Some("Lead"))?;
                    let other = c.other.expect("checked");
                    same(name_of(app, other).as_deref(), Some("Guitar R"))?;
                    if app.selection != Some(Selection::Track(other)) {
                        return Err("Guitar R wasn't selected".into());
                    }
                    undo_steps(app, c, 3 + usize::from(c.bus.is_some()), "Rename track")
                },
            ),
        ]);
    }
    steps
}

fn volume_steps(track: TrackId) -> Vec<Step> {
    let volume = value_edit::level_id(Place::Header, Some(track), Level::Volume);
    let pan = value_edit::level_id(Place::Header, Some(track), Level::Pan);
    let inspector = value_edit::level_id(Place::Inspector, Some(track), Level::Volume);
    let master = value_edit::level_id(Place::Header, None, Level::Volume);
    let away = crate::inspector::kind_id();
    // Type `text` into `field` and press `key`.
    let typed = |field: Id, text: &'static str, key: Key| {
        [
            Step::Click(field),
            Step::Open(field, true),
            Step::Type(text),
            Step::Press(key),
            Step::Open(field, false),
        ]
    };
    let mut steps = vec![
        Step::Click(volume),
        Step::Open(volume, true),
        Step::Type("-6.5 dB"),
        Step::Shot,
        Step::Press(Key::Enter),
        Step::Open(volume, false),
        Step::Expect(
            "-6.5 dB typed and Enter: the track's volume",
            |app, ctx, c| {
                gain_is(app, c, -6.5)?;
                let header = value_edit::level_id(Place::Header, Some(c.track), Level::Volume);
                let inspector =
                    value_edit::level_id(Place::Inspector, Some(c.track), Level::Volume);
                same(drawn(ctx, header).as_deref(), Some("-6.5"))?;
                same(drawn(ctx, inspector).as_deref(), Some("-6.5"))?;
                undo_steps(app, c, 1, "Volume")
            },
        ),
        // Bad text: refused on Enter, then Escape.
        Step::Click(volume),
        Step::Open(volume, true),
        Step::Type("loud"),
        Step::Press(Key::Enter),
        Step::Expect("\"loud\" is refused, the field kept open", |app, ctx, c| {
            let id = value_edit::level_id(Place::Header, Some(c.track), Level::Volume);
            if !value_edit::is_open(ctx, id) || !value_edit::refused(ctx, id) {
                return Err("the field closed, or isn't marked refused".into());
            }
            gain_is(app, c, -6.5)
        }),
        Step::Press(Key::Escape),
        Step::Open(volume, false),
        Step::Expect("Escape kept the volume", |app, _, c| {
            gain_is(app, c, -6.5)?;
            undo_steps(app, c, 1, "Volume")
        }),
    ];
    steps.extend(typed(volume, "-inf", Key::Enter));
    steps.push(Step::Expect("-inf is the fader's bottom", |app, _, c| {
        gain_is(app, c, -60.0)
    }));
    steps.extend(typed(volume, "+30", Key::Enter));
    steps.push(Step::Expect(
        "+30 is clamped to the fader's top, each value a step",
        |app, _, c| {
            gain_is(app, c, 12.0)?;
            undo_steps(app, c, 3, "Volume")
        },
    ));
    steps.extend(typed(pan, "L30", Key::Enter));
    steps.push(Step::Expect("L30 typed: the pan", |app, ctx, c| {
        let pan = app.session.project.track(c.track).map(|t| t.mixer.pan);
        if pan != Some(-0.3) {
            return Err(format!("the pan is {pan:?}, not -0.3"));
        }
        let id = value_edit::level_id(Place::Header, Some(c.track), Level::Pan);
        same(drawn(ctx, id).as_deref(), Some("L30"))?;
        undo_steps(app, c, 4, "Pan")
    }));
    // The inspector's, committed by clicking away.
    steps.extend([
        Step::Click(inspector),
        Step::Open(inspector, true),
        Step::Type("-3"),
        Step::Click(away),
        Step::Open(inspector, false),
        Step::Expect("-3 typed in the inspector, clicked away", |app, _, c| {
            gain_is(app, c, -3.0)?;
            undo_steps(app, c, 5, "Volume")
        }),
    ]);
    steps.extend(typed(master, "-2", Key::Enter));
    steps.push(Step::Expect("the master's volume typed", |app, _, c| {
        let gain = app.session.project.master.mixer.gain_db;
        if gain != -2.0 {
            return Err(format!("the master's volume is {gain}, not -2"));
        }
        undo_steps(app, c, 6, "Master volume")
    }));
    steps
}

fn same(found: Option<&str>, wanted: Option<&str>) -> Result<(), String> {
    if found == wanted {
        Ok(())
    } else {
        Err(format!("found {found:?}, wanted {wanted:?}"))
    }
}

fn name_of(app: &DawApp, track: TrackId) -> Option<String> {
    app.session.project.track(track).map(|t| t.name.clone())
}

fn drawn(ctx: &egui::Context, id: Id) -> Option<String> {
    value_edit::drawn(ctx, id).map(|d| d.text)
}

/// The track is called `name` in the project, its header and the inspector.
fn name_everywhere(app: &DawApp, ctx: &egui::Context, c: &Check, name: &str) -> Result<(), String> {
    same(name_of(app, c.track).as_deref(), Some(name)).map_err(|e| format!("project: {e}"))?;
    let header = value_edit::name_id(Place::Header, Some(c.track));
    same(drawn(ctx, header).as_deref(), Some(name)).map_err(|e| format!("header: {e}"))?;
    let inspector = value_edit::name_id(Place::Inspector, None);
    same(drawn(ctx, inspector).as_deref(), Some(name)).map_err(|e| format!("inspector: {e}"))
}

fn gain_is(app: &DawApp, c: &Check, db: f32) -> Result<(), String> {
    let gain = app.session.project.track(c.track).map(|t| t.mixer.gain_db);
    if gain == Some(db) {
        Ok(())
    } else {
        Err(format!("the volume is {gain:?}, not {db}"))
    }
}

/// `count` undo steps since the check began, the last named `last`.
fn undo_steps(app: &DawApp, c: &Check, count: usize, last: &str) -> Result<(), String> {
    let undo = app.session.history().0;
    let made = undo.len().saturating_sub(c.undo_before);
    if made != count || undo.first().map(String::as_str) != Some(last) {
        return Err(format!(
            "{made} undo step(s) since the start, the last {:?}; wanted {count}, the last {last:?}",
            undo.first()
        ));
    }
    Ok(())
}

impl Check {
    /// The screenshot is due: the field open with its text typed.
    pub fn shot_due(&self) -> bool {
        matches!(self.steps.get(self.at), Some(Step::Shot))
    }

    fn describe(&self) -> String {
        let what = match self.steps.get(self.at) {
            Some(Step::ClickAt(what, _)) => format!("a click on {what}"),
            Some(Step::RightClickAt(what, _)) => format!("a right-click on {what}"),
            Some(Step::DoubleClickAt(what, _)) => format!("a double-click on {what}"),
            Some(Step::Send(what, _)) => format!("sending {what}"),
            Some(Step::PointerTo(what, _)) => format!("the pointer to {what}"),
            Some(Step::PointerDown(what, _)) => format!("a press on {what}"),
            Some(Step::PointerUp) => "letting go".to_string(),
            Some(Step::Do(what, _)) => what.to_string(),
            Some(Step::Click(_)) => "a click".to_string(),
            Some(Step::RightClick(_)) => "a right-click".to_string(),
            Some(Step::DoubleClick(_)) => "a double-click".to_string(),
            Some(Step::Type(t)) => format!("typing {t:?}"),
            Some(Step::Press(k)) => format!("pressing {k:?}"),
            Some(Step::Open(_, true)) => "the field opening".to_string(),
            Some(Step::Open(_, false)) => "the field closing".to_string(),
            Some(Step::Shot) => "the screenshot".to_string(),
            Some(Step::Expect(what, _) | Step::Wait(what, _)) => what.to_string(),
            None => "the end".to_string(),
        };
        format!("step {} ({what})", self.at + 1)
    }

    /// The current step has waited too long.
    fn timed_out(&self) -> bool {
        match self.steps.get(self.at) {
            Some(Step::Wait(..)) => self.step_started.elapsed() > WAIT_TIMEOUT,
            _ => self.waited > TIMEOUT,
        }
    }

    fn next(&mut self) {
        self.at += 1;
        self.waited = 0;
        self.step_started = Instant::now();
        self.waiting = None;
    }
}

/// The screenshot was asked for (`take_screenshot_when_ready`).
pub fn shot_taken(app: &mut DawApp) {
    if let Some(c) = &mut app.edit_check {
        c.shot_at = Some(c.frame);
    }
}

fn click(raw: &mut egui::RawInput, at: egui::Pos2) {
    press(raw, at, PointerButton::Primary);
}

fn press(raw: &mut egui::RawInput, at: egui::Pos2, button: PointerButton) {
    raw.events.push(Event::PointerMoved(at));
    for pressed in [true, false] {
        raw.events.push(Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
}

/// The primary button pressed or let go at `at`.
fn primary(at: egui::Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// What a `DoubleClickAt`'s first press needs from the app: where it goes, the playhead it is
/// to put there, and the playhead and the clips as it goes in.
struct Pressed {
    place: Option<egui::Pos2>,
    expected: Option<u64>,
    position: u64,
    clips: Vec<Clip>,
}

/// One frame of a `DoubleClickAt` (26.2), the events as egui-winit sends a real double-click:
/// `PointerMoved` and the press in one frame, the release in the next, and the second press
/// once the playhead's handle has been drawn where the first click put it (`handle_x`, last
/// frame's, within 3 points of the click: what took the second press from the clip before
/// 26.2) and [`SECOND_PRESS_FROM`] has passed, or at [`SECOND_PRESS_BY`]; then its release,
/// and the step is done.
fn double_click(c: &mut Check, raw: &mut egui::RawInput, first: Pressed, handle_x: Option<f32>) {
    match c.double.as_mut() {
        None => {
            let apart = c.clicked_at.is_none_or(|t| t.elapsed() >= CLICKS_APART);
            let Some(at) = first.place.filter(|_| apart) else {
                return;
            };
            raw.events.push(Event::PointerMoved(at));
            raw.events.push(primary(at, true));
            c.pointer = Some(at);
            c.clicked = Some(at);
            c.expected = first.expected;
            c.position_before = first.position;
            c.held = first.clips;
            c.double = Some(Double {
                at,
                phase: 1,
                first: Instant::now(),
                frame: c.frame,
            });
        }
        Some(d) if d.phase == 1 && c.frame > d.frame => {
            raw.events.push(primary(d.at, false));
            d.phase = 2;
            d.frame = c.frame;
        }
        Some(d) if d.phase == 2 && c.frame > d.frame + 1 => {
            let under = handle_x.is_some_and(|x| (x - d.at.x).abs() <= 3.0);
            let since = d.first.elapsed();
            if (under && since >= SECOND_PRESS_FROM) || since >= SECOND_PRESS_BY {
                println!(
                    "the second press {:.0} ms after the first, the playhead {} the pointer",
                    d.first.elapsed().as_secs_f64() * 1000.0,
                    if under { "under" } else { "not under" }
                );
                raw.events.push(primary(d.at, true));
                d.phase = 3;
                d.frame = c.frame;
            }
        }
        Some(d) if d.phase == 3 && c.frame > d.frame => {
            raw.events.push(primary(d.at, false));
            let took = d.first.elapsed();
            c.double = None;
            c.clicked_at = Some(Instant::now());
            if took >= Duration::from_millis(300) {
                c.failures.push(format!(
                    "the double-click took {} ms, past egui's double-click time: the check \
                     couldn't make one",
                    took.as_millis()
                ));
            }
            c.next();
        }
        _ => {}
    }
}

/// Before a frame (`raw_input_hook`): the step's click, keys or text.
pub fn input(app: &mut DawApp, ctx: &egui::Context, raw: &mut egui::RawInput) {
    // The pointer gone (the playhead drag check): egui told so once, as egui-winit does on
    // Windows' mouse-leave message, and then no pointer at all until the next `PointerTo`.
    if let Some(c) = app.edit_check.as_mut().filter(|c| !c.reported) {
        if c.gone.is_some() && !c.gone_sent {
            raw.events.push(Event::PointerGone);
            c.gone_sent = true;
        }
    }
    let Some(c) = &app.edit_check else {
        return;
    };
    // A pointer the check drives stays where it put it every frame, whatever the real mouse
    // over the window says (winit's own moves, or the pointer leaving the window).
    if let Some(at) = c.pointer.filter(|_| !c.reported) {
        raw.events.push(Event::PointerMoved(at));
    }
    // And no modifier the real keyboard holds (an Alt-Tab away would make a drag a stretch),
    // only those the check holds.
    if !c.reported {
        raw.events.push(Event::ModifiersChanged(c.modifiers));
    }
    if c.frame < START || c.waited < SETTLE || c.reported {
        return;
    }
    // What the step needs from the app, worked out before the check is borrowed to step on.
    let (place, events) = match c.steps.get(c.at) {
        Some(
            Step::ClickAt(_, spot)
            | Step::RightClickAt(_, spot)
            | Step::DoubleClickAt(_, spot)
            | Step::PointerTo(_, spot)
            | Step::PointerDown(_, spot),
        ) => (spot(app, c), None),
        Some(Step::Send(_, events)) => (None, Some(events(app))),
        _ => (None, None),
    };
    let pressing = matches!(
        c.steps.get(c.at),
        Some(
            Step::PointerDown(..)
                | Step::ClickAt(..)
                | Step::RightClickAt(..)
                | Step::DoubleClickAt(..)
        )
    );
    let clips_now = if pressing { all_clips(app) } else { Vec::new() };
    let position = app.session.position();
    // Where the playhead's handle was drawn last frame, and where a click at `place` puts it.
    // In the piano roll (26.4): its line, and its own placing.
    let (handle_x, expected) = if c.kind == Kind::PianoRollClick {
        (
            piano_drawn(app).and_then(|d| d.line_x),
            place.and_then(|at| piano_playhead_for(app, c, at.x)),
        )
    } else {
        (
            app.playhead_grab.map(|(p, _)| p.x),
            place.and_then(|at| playhead_for(app, at.x)),
        )
    };
    let c = app.edit_check.as_mut().expect("checked");
    let spot = |id: Id| value_edit::drawn(ctx, id).map(|d| d.rect.center());
    match c.steps.get(c.at) {
        Some(step @ (Step::ClickAt(..) | Step::RightClickAt(..))) => {
            let button = match step {
                Step::RightClickAt(..) => PointerButton::Secondary,
                _ => PointerButton::Primary,
            };
            let apart = c.clicked_at.is_none_or(|t| t.elapsed() >= CLICKS_APART);
            if let Some(at) = place.filter(|_| apart) {
                press(raw, at, button);
                c.clicked_at = Some(Instant::now());
                c.clicked = Some(at);
                c.position_before = position;
                c.held = clips_now;
                c.next();
            }
        }
        Some(Step::DoubleClickAt(..)) => {
            let first = Pressed {
                place,
                expected,
                position,
                clips: clips_now,
            };
            double_click(c, raw, first, handle_x);
        }
        Some(Step::Send(..)) => {
            raw.events.extend(events.unwrap_or_default());
            c.next();
        }
        Some(Step::PointerTo(..)) => {
            if let Some(at) = place {
                raw.events.push(Event::PointerMoved(at));
                c.pointer = Some(at);
                c.gone = None;
                if let Some(w) = &mut c.drag {
                    w.calm = CALM;
                }
                c.next();
            }
        }
        Some(Step::PointerDown(..)) => {
            if let Some(at) = place {
                raw.events.push(Event::PointerMoved(at));
                raw.events.push(Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                });
                c.grab = Some(at);
                c.pointer = Some(at);
                c.held = clips_now;
                c.position_before = position;
                c.next();
            }
        }
        Some(Step::PointerUp) => {
            // A watched drag ends with this frame's release: its verdict now.
            end_drag_watch(c);
            if let Some(at) = c.pointer {
                raw.events.push(Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                });
            }
            c.next();
        }
        Some(Step::Click(id)) => {
            // Apart from the last click, or egui would take the two for a double-click.
            let apart = c.clicked_at.is_none_or(|t| t.elapsed() >= CLICKS_APART);
            if let Some(at) = spot(*id).filter(|_| apart) {
                click(raw, at);
                c.clicked_at = Some(Instant::now());
                c.next();
            }
        }
        Some(Step::RightClick(id)) => {
            let apart = c.clicked_at.is_none_or(|t| t.elapsed() >= CLICKS_APART);
            if let Some(at) = spot(*id).filter(|_| apart) {
                press(raw, at, PointerButton::Secondary);
                c.clicked_at = Some(Instant::now());
                c.next();
            }
        }
        Some(Step::DoubleClick(id)) => {
            // Two clicks two frames apart, well inside egui's double-click time.
            if let Some(at) = spot(*id) {
                let apart = c.clicked_at.is_none_or(|t| t.elapsed() >= CLICKS_APART);
                match c.first_click {
                    None if apart => {
                        click(raw, at);
                        c.first_click = Some(c.frame);
                    }
                    Some(first) if c.frame >= first + 2 => {
                        click(raw, at);
                        c.first_click = None;
                        c.clicked_at = Some(Instant::now());
                        c.next();
                    }
                    _ => {}
                }
            }
        }
        Some(Step::Type(text)) => {
            raw.events.push(Event::Text((*text).to_string()));
            c.next();
        }
        Some(Step::Press(key)) => {
            for pressed in [true, false] {
                raw.events.push(Event::Key {
                    key: *key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                });
            }
            c.next();
        }
        _ => {}
    }
}

/// After a frame (`ui`): waits and checks; once every step is through, the verdict and close.
pub fn frame_ends(app: &mut DawApp, ctx: &egui::Context) {
    let Some(c) = &mut app.edit_check else {
        return;
    };
    c.frame += 1;
    c.waited += 1;
    if c.reported || c.frame < START {
        return;
    }
    watch_rows(app);
    watch_drag(app, ctx);
    let Some(c) = &mut app.edit_check else {
        return;
    };
    let mut advance = false;
    match c.steps.get(c.at) {
        Some(Step::Open(id, open)) => advance = value_edit::is_open(ctx, *id) == *open,
        Some(Step::Shot) => {
            advance = c.no_shot || c.shot_at.is_some_and(|at| c.frame >= at + HOLD_SHOT);
        }
        Some(Step::Do(what, act)) => {
            let (what, act) = (*what, *act);
            act(app);
            println!("ok: {what}");
            advance = true;
        }
        Some(Step::Expect(what, test)) => {
            let (what, test) = (*what, *test);
            let result = {
                let c = app.edit_check.as_ref().expect("checked");
                test(app, ctx, c)
            };
            let c = app.edit_check.as_mut().expect("checked");
            match result {
                Ok(()) => println!("ok: {what}"),
                Err(e) => c.failures.push(format!("{what}: {e}")),
            }
            advance = true;
        }
        Some(Step::Wait(what, test)) => {
            let (what, test) = (*what, *test);
            let result = {
                let c = app.edit_check.as_ref().expect("checked");
                test(app, ctx, c)
            };
            let c = app.edit_check.as_mut().expect("checked");
            match result {
                Ok(()) => {
                    println!("ok: {what}");
                    advance = true;
                }
                Err(e) => c.waiting = Some(e),
            }
        }
        _ => {}
    }
    let c = app.edit_check.as_mut().expect("checked");
    if advance {
        c.next();
    } else if c.at < c.steps.len() && c.timed_out() {
        let step = c.describe();
        let seen = match c.steps.get(c.at) {
            Some(
                Step::Click(id) | Step::RightClick(id) | Step::DoubleClick(id) | Step::Open(id, _),
            ) => match value_edit::drawn(ctx, *id) {
                Some(d) => format!("; its place was drawn at {:?}", d.rect),
                None => "; its place was never drawn".to_string(),
            },
            Some(Step::Wait(..)) => match &c.waiting {
                Some(e) => format!("; last seen: {e}"),
                None => String::new(),
            },
            _ => String::new(),
        };
        let limit = match c.steps.get(c.at) {
            Some(Step::Wait(..)) => format!("{} s", WAIT_TIMEOUT.as_secs()),
            _ => format!("{TIMEOUT} frames"),
        };
        c.failures
            .push(format!("{step} never happened ({limit}){seen}"));
        c.at = c.steps.len();
    }
    if c.at >= c.steps.len() && !c.reported {
        c.reported = true;
        let what = match c.kind {
            Kind::Rename => "rename",
            Kind::Volume => "typed volumes",
            Kind::RecordMenu => "Record menu",
            Kind::CountIn => "count-in",
            Kind::ClipCopyPaste => "clip copy and paste",
            Kind::ClipEdges => "clip edges",
            Kind::ClipPlayhead => "clip click playhead",
            Kind::TrackZoom => "track zoom",
            Kind::PlayheadDrag => "playhead drag",
            Kind::PianoRollClick => "piano roll click",
            Kind::HelpManual => "Help > Manual and F1",
        };
        if c.failures.is_empty() {
            println!("the {what} check: {} steps", c.steps.len());
            println!("RESULT: PASS");
        } else {
            for f in &c.failures {
                println!("FAIL: {f}");
            }
            FAILED.store(true, Ordering::Relaxed);
        }
        app.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}
