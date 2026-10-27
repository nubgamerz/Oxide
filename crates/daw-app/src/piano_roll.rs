//! The piano roll: a MIDI clip's notes on a grid of pitch rows against musical time, in a
//! panel under the arrangement.
//!
//! Time runs in ticks from the clip's start (a straight line, whatever the tempo); grid lines
//! come from the tempo map's bars and beats, and snapping is exact in ticks
//! ([`TempoMap::snap_tick`]). Every edit goes through the project's note functions, is one
//! undo step (a drag included, by its own merge key), and marks the graph dirty so it is heard.

use crate::app::DawApp;
use crate::theme;
use daw_core::grid::{Division, Grid, Rounding};
use daw_core::midi_clip::{MidiClip, Note, NoteEdge, NoteId};
use daw_core::project::{ClipId, TrackId};
use daw_core::tempo::LineKind;
use daw_core::tempo::TempoMap;
use egui::{pos2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind};
use std::collections::BTreeSet;

/// One pitch row, in points.
const ROW: f32 = 12.0;
const KEYS_WIDTH: f32 = 56.0;
/// How far across the keyboard the black keys reach.
const BLACK_KEY: f32 = 0.6;
const VELOCITY_HEIGHT: f32 = 64.0;
/// The scroll bar between the grid and the velocity lane (20.2).
const SCROLL_BAR: f32 = 12.0;
/// The scroll bar's thumb is never narrower than this, in points.
const MIN_THUMB: f32 = 16.0;
/// The ruler over the grid.
const RULER: f32 = 18.0;
/// Room left of the clip's start when scrolled all the way back, so its first bar line and
/// notes stand clear of the keyboard.
const MARGIN: f32 = 12.0;
/// Within this many points of a note's end (or start), a drag resizes it.
const EDGE: f32 = 5.0;
const SIXTEENTH: Division = Division::note(16);

/// What the editor shows and how. Lives in [`DawApp::piano_roll`] while open.
pub struct PianoRoll {
    pub clip: ClipId,
    /// Ticks per point horizontally.
    ticks_per_px: f64,
    /// Clip-relative tick at the grid's left edge.
    scroll_tick: f64,
    /// The pitch whose row starts at the grid's top edge (fractional while scrolling).
    top_pitch: f32,
    pub selected: BTreeSet<NoteId>,
    /// Snapping on (the menu's Off turns it off here only; Alt while dragging too). The
    /// division is the song's grid (17.4), one setting with the Multitrack's.
    snap_on: bool,
    /// The grid's division in effect this frame: the song's fixed one, or Auto's at this
    /// zoom (resolved once a frame at the view's first bar).
    shown: Division,
    drag: Option<Drag>,
    /// The key held down on the keyboard (sounding).
    held_key: Option<u8>,
    /// The note a drag holds sounding (20.3).
    preview: NotePreview,
    /// Counts gestures, so each drag merges into one undo step of its own.
    gesture: u64,
    /// The pointer was over the editor last frame: its keys (Delete, Ctrl+A, Q) win.
    pub hovered: bool,
    /// The pitch to centre the view on once the grid's height is known (the first frame).
    centre: Option<f32>,
    /// For the screenshot check: a key to put the pointer on, and where that is this frame
    /// (`DawApp::raw_input_hook` moves the pointer there).
    pub point_at: Option<u8>,
    pub pointer: Option<Pos2>,
    /// The playhead's sample last frame: when it jumps elsewhere while stopped (the
    /// arrangement moved it) and lands off screen, the view goes to it.
    last_position: Option<u64>,
    /// A drag of the playhead (its line or the ruler) under way (26.3): where it put the
    /// playhead and where the pointer was last seen.
    playhead_drag: Option<crate::arrange::PlayheadDrag>,
    /// For the screenshot check: where the ruler is this frame, to click it.
    pub ruler: Option<Rect>,
    /// For the driven checks: where the editor's parts and its playhead line were drawn this
    /// frame.
    pub(crate) drawn: Option<Drawn>,
}

/// Where the piano roll's parts were drawn this frame (`PianoRoll::drawn`), as the driven
/// `--show piano-roll-click` finds its places: the note grid, the keyboard and the velocity
/// lane, the view across them, and the playhead's line (none while off screen).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Drawn {
    pub grid: Rect,
    pub keys: Rect,
    pub lane: Rect,
    /// The clip-relative tick at the grid's left edge, and ticks a point.
    pub start_tick: f64,
    pub ticks_per_px: f64,
    /// The pitch whose row starts at the grid's top edge.
    pub top_pitch: f32,
    pub line_x: Option<f32>,
}

impl Drawn {
    /// The middle of pitch `pitch`'s row at clip-relative tick `rel`.
    pub fn point(&self, rel: f64, pitch: u8) -> Pos2 {
        pos2(
            self.grid.left() + ((rel - self.start_tick) / self.ticks_per_px) as f32,
            self.grid.top() + (self.top_pitch - f32::from(pitch)) * ROW + ROW / 2.0,
        )
    }

    /// The clip-relative tick under `x`.
    pub fn tick(&self, x: f32) -> f64 {
        self.start_tick + f64::from(x - self.grid.left()) * self.ticks_per_px
    }
}

enum Drag {
    /// Move the selected notes; `grab` is the note under the pointer, `original` the notes
    /// before the drag began.
    Move {
        original: Vec<Note>,
        grab: Note,
        from: Pos2,
    },
    Resize {
        original: Vec<Note>,
        grab: Note,
        from: Pos2,
        edge: NoteEdge,
    },
    /// A selection box from here.
    Select {
        from: Pos2,
    },
    Velocity {
        original: Vec<Note>,
        grab: NoteId,
    },
}

impl PianoRoll {
    /// Open `clip`, scrolled to its notes (or to middle C).
    pub fn open(clip: &MidiClip) -> Self {
        let (low, high) = clip
            .notes
            .iter()
            .fold((u8::MAX, 0u8), |(l, h), n| (l.min(n.pitch), h.max(n.pitch)));
        let middle = if low <= high {
            (f32::from(low) + f32::from(high)) / 2.0
        } else {
            60.0
        };
        let ticks_per_px = 8.0;
        Self {
            clip: clip.id,
            ticks_per_px,
            scroll_tick: -f64::from(MARGIN) * ticks_per_px,
            top_pitch: (middle + 12.0).min(127.0),
            selected: BTreeSet::new(),
            snap_on: true,
            shown: Division::note(8),
            drag: None,
            held_key: None,
            preview: NotePreview::default(),
            gesture: 0,
            hovered: false,
            centre: Some(middle),
            point_at: None,
            pointer: None,
            last_position: None,
            playhead_drag: None,
            ruler: None,
            drawn: None,
        }
    }

    /// The division clicks and drags snap to (the editor's Snap on), or none.
    pub(crate) fn division(&self) -> Option<Division> {
        self.snap_on.then_some(self.shown)
    }

    /// The editor's Snap on or off, as its menu sets it (a driven check flips it).
    pub(crate) fn set_snap(&mut self, on: bool) {
        self.snap_on = on;
    }

    /// Zoom (ticks per point) and scroll (the clip-relative tick at the grid's left edge) set
    /// at once: the screenshot check's view of a clip scrolled partway.
    pub fn set_view(&mut self, ticks_per_px: f64, scroll_tick: f64) {
        self.ticks_per_px = ticks_per_px.clamp(0.5, 200.0);
        self.scroll_tick = scroll_tick.max(earliest(self.ticks_per_px));
    }
}

/// A note heard while it is dragged (20.3): struck when the drag starts, struck again only when
/// the drag takes it to another pitch (the old one released first), left ringing while it moves
/// along in time, released when the drag ends.
#[derive(Debug, Default)]
pub struct NotePreview {
    /// The track and pitch sounding.
    sounding: Option<(TrackId, u8)>,
}

/// What a gesture's step sends the instrument: release `off`, then strike `on` (pitch,
/// velocity).
#[derive(Debug, Default, PartialEq)]
pub struct PreviewSound {
    pub off: Option<(TrackId, u8)>,
    pub on: Option<(TrackId, u8, u8)>,
}

impl NotePreview {
    /// The drag now wants `want` (pitch, velocity) sounding on `track`, or nothing (it ended).
    pub fn follow(&mut self, track: TrackId, want: Option<(u8, u8)>) -> PreviewSound {
        let pitch = want.map(|(pitch, _)| (track, pitch));
        if pitch == self.sounding {
            return PreviewSound::default();
        }
        let off = self.sounding.take();
        self.sounding = pitch;
        PreviewSound {
            off,
            on: want.map(|(pitch, velocity)| (track, pitch, velocity)),
        }
    }

    /// Release what sounds (the editor closing mid-drag).
    pub fn release(&mut self) -> Option<(TrackId, u8)> {
        self.sounding.take()
    }
}

/// The furthest back the view scrolls: `MARGIN` before the clip's start.
fn earliest(ticks_per_px: f64) -> f64 {
    -f64::from(MARGIN) * ticks_per_px
}

/// The horizontal scroll bar's maths (20.2), in clip-relative ticks: the span the bar stands
/// for runs from the furthest back the view scrolls to a little past the clip's end and the
/// song's (whichever is later), or to the view's end if that reaches further, as the
/// Multitrack's bar spans the project.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollSpan {
    pub first: f64,
    pub last: f64,
}

impl ScrollSpan {
    /// The span for a clip `clip_length` ticks long in a song ending `song_end` ticks after the
    /// clip's start, viewed from `scroll_tick` across `visible` ticks.
    pub fn new(
        clip_length: f64,
        song_end: f64,
        scroll_tick: f64,
        visible: f64,
        ticks_per_px: f64,
    ) -> Self {
        let first = earliest(ticks_per_px).min(scroll_tick);
        // Room past the end, as the Multitrack's bar keeps: a quarter of a view.
        let end = clip_length.max(song_end) + visible / 4.0;
        Self {
            first,
            last: end.max(scroll_tick + visible).max(first + visible),
        }
    }

    fn ticks(&self) -> f64 {
        (self.last - self.first).max(1.0)
    }

    /// The thumb's left edge (from the bar's) and width, in points, on a bar `width` wide.
    pub fn thumb(&self, scroll_tick: f64, visible: f64, width: f32) -> (f32, f32) {
        let scale = f64::from(width) / self.ticks();
        let size = ((visible * scale) as f32).clamp(MIN_THUMB.min(width), width.max(0.0));
        let left = ((scroll_tick - self.first) * scale) as f32;
        (left.clamp(0.0, (width - size).max(0.0)), size)
    }

    /// The view's start once the thumb is dragged `dx` points along a bar `width` wide: no
    /// further back than the span's start, nor on past its end.
    pub fn dragged(&self, scroll_tick: f64, visible: f64, dx: f32, width: f32) -> f64 {
        let ticks = f64::from(dx) * self.ticks() / f64::from(width.max(1.0));
        self.clamp(scroll_tick + ticks, visible)
    }

    /// The view's start centred on the point `x` points along a bar `width` wide (a click
    /// beside the thumb jumps there).
    pub fn centred(&self, x: f32, visible: f64, width: f32) -> f64 {
        let at = self.first + f64::from(x) / f64::from(width.max(1.0)) * self.ticks();
        self.clamp(at - visible / 2.0, visible)
    }

    fn clamp(&self, scroll_tick: f64, visible: f64) -> f64 {
        scroll_tick.clamp(self.first, (self.last - visible).max(self.first))
    }
}

/// The division the piano roll shows and snaps to, from song tick `first` (the view's left
/// edge) at `ticks_per_px`: the song's grid in that bar (Auto's at this zoom).
pub(crate) fn shown_division(
    map: &TempoMap,
    grid: Grid,
    first: u64,
    ticks_per_px: f64,
) -> Division {
    map.division_in(grid, map.bar_of_tick(first), &|tick| tick / ticks_per_px)
}

/// The snap division in force: the song's grid (at this zoom), unless the editor's snap is off
/// or Alt is held.
fn snap_now(ui: &egui::Ui, state: &PianoRoll) -> Option<Division> {
    if ui.input(|i| i.modifiers.alt) {
        return None;
    }
    state.division()
}

/// Snap a clip-relative tick to the grid (in the song's bars, which the clip may start in the
/// middle of), or round it with snap off.
pub fn snap_rel(
    map: &TempoMap,
    clip_start: u64,
    rel: f64,
    snap: Option<Division>,
    rounding: Rounding,
) -> u64 {
    let rel = rel.max(0.0);
    match snap {
        Some(division) => {
            let at = clip_start + rel.round() as u64;
            map.snap_tick(at, division, rounding)
                .saturating_sub(clip_start)
        }
        None => rel.round() as u64,
    }
}

/// The grid step at `rel` in ticks: for drawing a note of one grid step, with snap off a
/// sixteenth.
fn step_at(map: &TempoMap, clip_start: u64, rel: u64, snap: Option<Division>) -> u64 {
    let at = clip_start + rel;
    let line = map.snap_tick(at, snap.unwrap_or(SIXTEENTH), Rounding::Previous);
    let next = map.snap_tick(line + 1, snap.unwrap_or(SIXTEENTH), Rounding::Next);
    next.saturating_sub(line).max(1)
}

/// The selected notes moved: the grabbed one's start goes to `to_tick` (already snapped) and
/// up `pitches` rows; the rest keep their places relative to it. Nothing goes before the
/// clip's start or off the keyboard.
pub fn moved(
    original: &[Note],
    selected: &BTreeSet<NoteId>,
    grab: &Note,
    to_tick: u64,
    pitches: i32,
) -> Vec<Note> {
    let chosen = || original.iter().filter(|n| selected.contains(&n.id));
    let earliest = chosen().map(|n| n.tick).min().unwrap_or(0);
    // The grabbed note goes no further back than keeps every selected note at or after 0.
    let to_tick = to_tick.max(grab.tick - earliest.min(grab.tick));
    let dtick = to_tick as i64 - grab.tick as i64;
    let low = chosen().map(|n| i32::from(n.pitch)).min().unwrap_or(0);
    let high = chosen().map(|n| i32::from(n.pitch)).max().unwrap_or(127);
    let pitches = pitches.clamp(-low, 127 - high);
    original
        .iter()
        .map(|n| {
            if !selected.contains(&n.id) {
                return *n;
            }
            Note {
                tick: (n.tick as i64 + dtick).max(0) as u64,
                pitch: (i32::from(n.pitch) + pitches) as u8,
                ..*n
            }
        })
        .collect()
}

/// "C#" for 61: the names agents read and write (`daw_core::midi_analysis`).
fn pitch_class(pitch: u8) -> &'static str {
    daw_core::midi_analysis::pitch_class_name(pitch, false)
}

/// "C4" for 60.
fn pitch_name(pitch: u8) -> String {
    daw_core::midi_analysis::note_name(pitch, false)
}

fn is_black(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

/// The grid lines of `division` from song tick `first` to `last`.
fn lines(map: &TempoMap, first: u64, last: u64, division: Division) -> Vec<u64> {
    let mut out = Vec::new();
    let mut tick = map.snap_tick(first, division, Rounding::Next);
    // Bounded, should a zoom ask for more lines than can be told apart anyway.
    while tick <= last && out.len() < 4096 {
        out.push(tick);
        tick = map.snap_tick(tick + 1, division, Rounding::Next);
    }
    out
}

/// Where things are on screen this frame.
#[derive(Clone, Copy)]
struct View {
    grid: Rect,
    start_tick: f64,
    ticks_per_px: f64,
    top_pitch: f32,
}

impl View {
    fn x(&self, rel: f64) -> f32 {
        self.grid.left() + ((rel - self.start_tick) / self.ticks_per_px) as f32
    }
    fn tick(&self, x: f32) -> f64 {
        self.start_tick + f64::from(x - self.grid.left()) * self.ticks_per_px
    }
    fn y(&self, pitch: u8) -> f32 {
        self.grid.top() + (self.top_pitch - f32::from(pitch)) * ROW
    }
    fn pitch(&self, y: f32) -> u8 {
        (self.top_pitch - (y - self.grid.top()) / ROW)
            .ceil()
            .clamp(0.0, 127.0) as u8
    }
    fn note_rect(&self, n: &Note) -> Rect {
        let x0 = self.x(n.tick as f64);
        let x1 = self.x((n.tick + n.length) as f64).max(x0 + 2.0);
        Rect::from_min_max(
            pos2(x0, self.y(n.pitch) + 1.0),
            pos2(x1, self.y(n.pitch) + ROW - 1.0),
        )
    }
    /// The song ticks (from the timeline start) across the grid.
    fn song_ticks(&self, clip: &MidiClip) -> (u64, u64) {
        let at = |x: f32| (clip.start as f64 + self.tick(x)).max(0.0) as u64;
        (at(self.grid.left()), at(self.grid.right()))
    }
    /// A song tick's distance across the screen, in points from any origin: the zoom the
    /// grid's lines are thinned by and Auto picks its division by.
    fn x_of_tick(&self) -> impl Fn(f64) -> f64 {
        let ticks_per_px = self.ticks_per_px;
        move |tick| tick / ticks_per_px
    }
    /// The x of a song tick.
    fn song_x(&self, clip: &MidiClip, tick: u64) -> f32 {
        self.x(tick as f64 - clip.start as f64)
    }
}

/// The note under `pos`, topmost first, and the edge `pos` is on, if it is on one. Short
/// notes keep a middle to grab: each edge takes at most a third.
fn note_at(notes: &[Note], view: &View, pos: Pos2) -> Option<(Note, Option<NoteEdge>)> {
    notes.iter().rev().find_map(|n| {
        let r = view.note_rect(n);
        let grip = EDGE.min(r.width() / 3.0);
        let edge = if pos.x >= r.right() - grip {
            Some(NoteEdge::End)
        } else if pos.x <= r.left() + grip {
            Some(NoteEdge::Start)
        } else {
            None
        };
        r.contains(pos).then_some((*n, edge))
    })
}

/// The key under `pos` on the keyboard. Right of a black key, the white key whose half
/// reaches into that row, as on a piano.
fn key_at(view: &View, keys: Rect, pos: Pos2) -> u8 {
    let pitch = view.pitch(pos.y);
    if !is_black(pitch) || pos.x <= keys.left() + KEYS_WIDTH * BLACK_KEY {
        return pitch;
    }
    // Black keys are never 0 or 127, so both neighbours exist.
    if pos.y < view.y(pitch) + ROW / 2.0 {
        pitch + 1
    } else {
        pitch - 1
    }
}

/// The editor's panel. Call while [`DawApp::piano_roll`] is open.
pub fn show(app: &mut DawApp, ui: &mut egui::Ui) {
    let Some(state) = app.piano_roll.as_ref() else {
        return;
    };
    let Some((track, clip)) = app
        .session
        .project
        .find_midi_clip(state.clip)
        .map(|(t, c)| (t, c.clone()))
    else {
        // The clip is gone (deleted, or undone away): a note its drag held is let go.
        if let Some((track, pitch)) = app.piano_roll.as_mut().and_then(|s| s.preview.release()) {
            app.session.preview_note_off(track, pitch);
        }
        app.piano_roll = None;
        return;
    };
    // Notes that left the clip (a split moved them to the next piece, or an undo took them)
    // drop out of the selection, so Delete and the other edits name only notes it has.
    if let Some(state) = app.piano_roll.as_mut().filter(|s| !s.selected.is_empty()) {
        let present: std::collections::HashSet<_> = clip.notes.iter().map(|n| n.id).collect();
        state.selected.retain(|id| present.contains(id));
    }
    let track_name = app
        .session
        .project
        .track(track)
        .map(|t| t.name.clone())
        .unwrap_or_default();
    let color = app
        .session
        .project
        .track(track)
        .map_or(theme::ACCENT, |t| theme::track_color(t.color));
    header(app, ui, &clip, &track_name);

    let area = ui.available_rect_before_wrap();
    let keys = Rect::from_min_max(
        pos2(area.left(), area.top() + RULER),
        pos2(
            area.left() + KEYS_WIDTH,
            area.bottom() - VELOCITY_HEIGHT - SCROLL_BAR,
        ),
    );
    let grid = Rect::from_min_max(
        pos2(keys.right(), keys.top()),
        pos2(area.right(), keys.bottom()),
    );
    let ruler = Rect::from_min_max(
        pos2(grid.left(), area.top()),
        pos2(area.right(), grid.top()),
    );
    // The scroll bar under the grid, the velocity lane under it.
    let bar = Rect::from_min_max(
        pos2(grid.left(), grid.bottom()),
        pos2(area.right(), grid.bottom() + SCROLL_BAR),
    );
    let lane = Rect::from_min_max(pos2(grid.left(), bar.bottom()), area.max);
    ui.allocate_rect(area, Sense::hover());
    ui.painter().rect_filled(
        Rect::from_min_max(area.min, pos2(keys.right(), keys.top())),
        0,
        theme::PANEL,
    );
    ui.painter().rect_filled(
        Rect::from_min_max(
            pos2(area.left(), bar.top()),
            pos2(keys.right(), bar.bottom()),
        ),
        0,
        theme::PANEL,
    );
    let map = app.tempo_map.clone();
    let song_grid = app.session.project.grid;
    // The song's end, in ticks from the clip's start: the scroll bar spans it too.
    let song_end = map.tick_at(app.session.content_end() as f64) - clip.start as f64;
    let state = app.piano_roll.as_mut().expect("checked above");
    state.hovered = ui.rect_contains_pointer(area);
    // On opening: the clip's pitches in the middle of the view, whole rows from the top.
    if let Some(centre) = state.centre.take() {
        let rows = grid.height() / ROW;
        state.top_pitch = (centre + rows / 2.0)
            .round()
            .clamp(rows.ceil().min(127.0), 127.0);
    }
    wheel(state, ui, grid);
    scroll_bar(state, ui, bar, clip.length as f64, song_end, grid.width());
    let view = View {
        grid,
        start_tick: state.scroll_tick,
        ticks_per_px: state.ticks_per_px,
        top_pitch: state.top_pitch,
    };
    state.pointer = state
        .point_at
        .map(|pitch| pos2(keys.right() - 8.0, view.y(pitch) + ROW / 2.0));
    state.shown = shown_division(&map, song_grid, view.song_ticks(&clip).0, view.ticks_per_px);
    state.drawn = Some(Drawn {
        grid,
        keys,
        lane,
        start_tick: view.start_tick,
        ticks_per_px: view.ticks_per_px,
        top_pitch: view.top_pitch,
        line_x: None,
    });
    let state = app.piano_roll.as_ref().expect("checked above");

    // The key under the pointer, and its row across the grid: where a click places a note.
    let hover = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| ui.rect_contains_pointer(keys.union(grid)) && keys.union(grid).contains(*p))
        .map(|p| {
            if keys.contains(p) {
                key_at(&view, keys, p)
            } else {
                view.pitch(p.y)
            }
        });
    crate::marks::put(ui, "piano_roll.ruler", ruler);
    crate::marks::put(ui, "piano_roll.keys", keys);
    crate::marks::put(ui, "piano_roll.grid", grid);
    crate::marks::put(ui, "piano_roll.scroll", bar);
    crate::marks::put(ui, "piano_roll.velocity", lane);
    draw_ruler(ui, &map, &clip, &view, ruler, song_grid);
    draw_grid(ui, &map, &clip, &view, song_grid, hover);
    draw_notes(ui, &clip, &view, &state.selected, color);
    keyboard(app, ui, track, keys, &view, hover);
    grid_input(app, ui, &map, &clip, track, &view);
    velocity_lane(app, ui, &clip, &view, lane, color);
    playhead(app, ui, &map, &clip, &view, ruler, lane.bottom());
}

/// Where song tick `tick` is on the piano roll's ruler this frame (x, and the ruler's middle
/// height), if it is open: the screenshot check clicks there.
pub fn ruler_point(app: &DawApp, tick: u64) -> Option<Pos2> {
    let state = app.piano_roll.as_ref()?;
    let ruler = state.ruler?;
    let (_, clip) = app.session.project.find_midi_clip(state.clip)?;
    let rel = tick as f64 - clip.start as f64;
    let x = ruler.left() + ((rel - state.scroll_tick) / state.ticks_per_px) as f32;
    Some(pos2(x, ruler.center().y))
}

/// A point on the grid's `division` some way into clip `clip` (the screenshot check's
/// target): its start plus a beat and a quarter, snapped to the division.
pub fn demo_target(map: &TempoMap, clip_start: u64, division: Division) -> u64 {
    map.snap_tick(clip_start + 1200, division, Rounding::Nearest)
}

/// The song sample the piano roll's ruler (or its playhead line) at `x` puts the playhead on:
/// the song tick under it, snapped to the grid (`snap`, in the song's bars; None: to the
/// tick), in samples through the tempo map. Never before the song's start.
fn ruler_sample(
    map: &TempoMap,
    clip_start: u64,
    view: &View,
    x: f32,
    snap: Option<Division>,
) -> u64 {
    let song = (clip_start as f64 + view.tick(x)).max(0.0).round() as u64;
    let tick = match snap {
        Some(division) => map.snap_tick(song, division, Rounding::Nearest),
        None => song,
    };
    map.sample_at_tick(tick as f64).round().max(0.0) as u64
}

/// The playhead to a click at `x` in the piano roll (26.4): its ruler, or its note grid on
/// empty space or a note. [`ruler_sample`] places it (the editor's Snap, to the nearest line;
/// Alt: to the tick), as a click in the Multitrack goes through `arrange::locate_click`; and
/// the arrangement shows it there too. Not the Multitrack's `clicked_sample`: the piano roll's
/// view runs in ticks (straight across tempo changes) and snaps by its own Snap. A refusal
/// (recording) is reported, as the Multitrack's is.
fn locate_click(
    app: &mut DawApp,
    map: &TempoMap,
    clip_start: u64,
    view: &View,
    x: f32,
    snap: Option<Division>,
) {
    let at = clicked_sample(map, clip_start, view, x, snap);
    let located = app.session.locate(at);
    app.report(located);
    app.reveal_playhead = true;
}

/// The song sample a click at `x` in the piano roll puts the playhead on ([`locate_click`]):
/// [`ruler_sample`] at `x` kept inside the grid's width (the ruler spans the same), so the
/// ruler and the note grid agree to the sample at every x.
fn clicked_sample(
    map: &TempoMap,
    clip_start: u64,
    view: &View,
    x: f32,
    snap: Option<Division>,
) -> u64 {
    let x = x.clamp(view.grid.left(), view.grid.right() - 1.0);
    ruler_sample(map, clip_start, view, x, snap)
}

/// Where a drag of the piano roll's playhead (its line or its ruler) with the pointer at `x`
/// puts it (26.3): as [`ruler_sample`] does, but kept in the grid's width
/// (`arrange::kept_on_screen`): a grid line nearest the pointer that lies past an edge would
/// leave the line off screen.
fn drag_sample(
    map: &TempoMap,
    clip_start: u64,
    view: &View,
    x: f32,
    snap: Option<Division>,
) -> u64 {
    let (left, right) = (view.grid.left(), view.grid.right());
    let x = x.clamp(left, right - 1.0);
    let plain = ruler_sample(map, clip_start, view, x, None);
    let Some(division) = snap else {
        return plain;
    };
    let song = (clip_start as f64 + view.tick(x)).max(0.0).round() as u64;
    crate::arrange::kept_on_screen(
        |rounding| {
            let tick = map.snap_tick(song, division, rounding);
            map.sample_at_tick(tick as f64).round().max(0.0) as u64
        },
        plain,
        |sample| view.x(map.tick_at(sample as f64) - clip_start as f64),
        left,
        right,
    )
}

/// The session's playhead (one for the song: the arrangement's) across the ruler, grid and
/// velocity lane. Click or drag the ruler, or drag the line, to move it, snapped to the
/// piano roll's grid (Alt: to the tick), the view scrolling near its edges. While playing
/// with Follow on, the view keeps it in sight; stopped, a jump elsewhere that lands off
/// screen (the arrangement moved it) brings the view to it. Moved from here, the arrangement
/// shows it too (`DawApp::reveal_playhead`). A drag lasts until the button is let go and
/// draws the playhead where it put it (26.3), as the Multitrack's does.
fn playhead(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    map: &TempoMap,
    clip: &MidiClip,
    view: &View,
    ruler: Rect,
    bottom: f32,
) {
    let Some(state) = app.piano_roll.as_ref() else {
        return;
    };
    let snap = snap_now(ui, state);
    let grid = view.grid;
    let full = Rect::from_min_max(pos2(grid.left(), ruler.top()), pos2(grid.right(), bottom));
    let rel_of = |sample: u64| map.tick_at(sample as f64) - clip.start as f64;
    let line_id = ui.scope_id().with("piano-playhead");
    let ruler_id = ui.scope_id().with("piano-ruler");
    let ctx = ui.ctx().clone();
    let line_held = ctx.is_being_dragged(line_id);
    if !line_held && !ctx.is_being_dragged(ruler_id) {
        if let Some(state) = app.piano_roll.as_mut() {
            state.playhead_drag = None;
        }
    }
    // While dragged, where the drag put it: the session's position follows a locate late.
    let drag = app.piano_roll.as_ref().and_then(|s| s.playhead_drag);
    let position = drag.map_or_else(|| app.session.position(), |d| d.target);
    let x = view.x(rel_of(position));
    let on_screen = (grid.left()..=grid.right()).contains(&x);

    let ruler_response = ui
        .interact(ruler, ruler_id, Sense::click_and_drag())
        .on_hover_text("Click or drag to move the playhead (snapped; Alt: not snapped)");
    // The line is drag-only and added after the grid, so it takes the presses where they
    // meet; but not while a press could be a double-click's second (26.4, 26.2's trap): a
    // click on the grid puts the line right under the pointer, and egui gives a press on a
    // drag-only widget over a clickable one to nobody's click, so the grid would never see
    // the double-click that draws a note. Nor over a note: one clicked (and so selected) has
    // the line on it, and the drag that follows moves or resizes the note, not the playhead.
    let waiting = crate::arrange::double_click_waits(app, ui);
    let over_note = ui
        .input(|i| i.pointer.latest_pos())
        .filter(|p| grid.contains(*p))
        .is_some_and(|p| note_at(&clip.notes, view, p).is_some());
    let takes_presses = line_held || !(waiting || over_note);
    // Interacted wherever the drag is once it holds the line, so it is never dropped.
    let line_response = ((on_screen && takes_presses) || line_held).then(|| {
        let x = x.clamp(grid.left(), grid.right());
        let grab = Rect::from_x_y_ranges(x - 3.0..=x + 3.0, grid.top()..=bottom);
        ui.interact(grab, line_id, Sense::drag())
    });
    if line_response
        .as_ref()
        .is_some_and(|r| r.hovered() || r.dragged())
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
    }
    let dragging = ruler_response.dragged() || line_response.as_ref().is_some_and(|r| r.dragged());
    // A click on the ruler: the playhead there, as a click on the grid puts it (26.4).
    if ruler_response.clicked() {
        if let Some(p) = ruler_response.interact_pointer_pos() {
            locate_click(app, map, clip.start, view, p.x, snap);
        }
    }
    // Dragging: egui's pointer (a response's own position is gone for the rest of a drag
    // once egui loses the pointer), or the last one seen.
    let pointer_x = if dragging {
        let seen = ui.input(|i| i.pointer.latest_pos()).map(|p| p.x);
        seen.or(drag.map(|d| d.pointer_x))
    } else {
        None
    };
    let mut drawn = None;
    if let Some(px) = pointer_x {
        // Near or past either edge, the view scrolls with it (15.6's rule).
        let speed = crate::zoom::edge_scroll(px, grid.left(), grid.right());
        let mut view = *view;
        if speed != 0.0 {
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            if let Some(state) = app.piano_roll.as_mut() {
                state.scroll_tick = (state.scroll_tick
                    + f64::from(speed * dt) * state.ticks_per_px)
                    .max(-f64::from(MARGIN) * state.ticks_per_px);
                view.start_tick = state.scroll_tick;
            }
            ui.ctx().request_repaint();
        }
        let at = drag_sample(map, clip.start, &view, px, snap);
        let mut target = position;
        if at != position {
            let located = app.session.locate(at);
            if located.is_ok() {
                target = at;
            }
            app.report(located);
            app.reveal_playhead = true;
        }
        if let Some(state) = app.piano_roll.as_mut() {
            state.playhead_drag = Some(crate::arrange::PlayheadDrag {
                target,
                pointer_x: px,
            });
        }
        drawn = Some(view.x(rel_of(target)).clamp(grid.left(), grid.right()));
    }

    // Keep it in sight: following while playing, or after a jump while stopped; never while
    // dragged, which moves the view itself.
    let position = app.session.position();
    let playing = app.session.is_playing();
    let follow = app.follow_playhead && playing;
    if let Some(state) = app.piano_roll.as_mut() {
        let jumped = !playing && state.last_position.is_some_and(|last| last != position);
        state.last_position = Some(position);
        let x = view.x(rel_of(position));
        let visible = f64::from(grid.width()) * state.ticks_per_px;
        let behind = x < grid.left();
        let ahead = x > grid.left() + grid.width() * if follow { 0.9 } else { 1.0 };
        if !dragging && (follow || jumped) && (behind || ahead) {
            state.scroll_tick =
                (rel_of(position) - visible * 0.1).max(-f64::from(MARGIN) * state.ticks_per_px);
        }
        state.ruler = Some(ruler);
    }

    // The line and its head.
    let x = drawn.unwrap_or_else(|| view.x(rel_of(position)));
    let shown = (grid.left()..=grid.right()).contains(&x);
    if let Some(d) = app.piano_roll.as_mut().and_then(|s| s.drawn.as_mut()) {
        d.line_x = shown.then_some(x);
    }
    if shown {
        let painter = ui.painter_at(full);
        painter.vline(
            x,
            full.y_range(),
            Stroke::new(if dragging { 2.5 } else { 1.5 }, theme::PLAYHEAD),
        );
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(x - 5.0, ruler.top()),
                pos2(x + 5.0, ruler.top()),
                pos2(x, ruler.top() + 7.0),
            ],
            theme::PLAYHEAD,
            Stroke::NONE,
        ));
    }
}

fn header(app: &mut DawApp, ui: &mut egui::Ui, clip: &MidiClip, track_name: &str) {
    let bar = ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{}  ·  {track_name}", clip.name)).strong());
        ui.label(
            egui::RichText::new(format!("{} notes", clip.notes.len()))
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.separator();
        let Some(state) = app.piano_roll.as_mut() else {
            return;
        };
        let snap_label = ui.label("Snap");
        let grid = app.session.project.grid;
        let shown = state.shown;
        let text = match (state.snap_on, grid) {
            (false, _) => "Off".to_string(),
            (true, Grid::Auto) => format!("Auto ({shown})"),
            (true, Grid::Fixed(d)) => d.to_string(),
        };
        let mut picked = None;
        let snap = egui::ComboBox::from_id_salt("piano-roll-snap")
            .width(90.0)
            .selected_text(text)
            .show_ui(ui, |ui| {
                if ui
                    .add(egui::Button::selectable(!state.snap_on, "Off"))
                    .on_hover_text("No snapping here (Alt while dragging does it once)")
                    .clicked()
                {
                    state.snap_on = false;
                }
                ui.separator();
                if let Some(grid) = crate::arrange::grid_choices(ui, grid, shown) {
                    picked = Some(grid);
                }
            })
            .response;
        crate::marks::put(ui, "piano_roll.snap", snap_label.rect.union(snap.rect));
        snap.on_hover_text("The song's grid, as the Multitrack's ruler sets it: one setting");
        if let Some(grid) = picked {
            state.snap_on = true;
            let done = app.session.set_grid(grid);
            app.report(done);
        }
        let Some(state) = app.piano_roll.as_ref() else {
            return;
        };
        let division = state.division();
        let selected: Vec<NoteId> = state.selected.iter().copied().collect();
        let which = if selected.is_empty() {
            "all notes"
        } else {
            "the selected notes"
        };
        let quantize_button = ui.add_enabled(division.is_some(), egui::Button::new("Quantize"));
        crate::marks::put(ui, "piano_roll.quantize", quantize_button.rect);
        if quantize_button
            .on_hover_text(format!("Snap the starts of {which} to the grid (Q)"))
            .clicked()
        {
            quantize(app, clip.id, &selected, division);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("✕")
                .on_hover_text("Close the piano roll")
                .clicked()
            {
                app.piano_roll = None;
            }
            ui.label(
                egui::RichText::new(
                    "Double-click: draw · Drag: move · Edges: resize · Alt: no snap · \
                     Right-click/Del: erase · Ctrl+wheel: zoom",
                )
                .small()
                .color(theme::TEXT_DIM),
            );
        });
    });
    crate::marks::put(ui, "piano_roll.toolbar", bar.response.rect);
}

fn quantize(app: &mut DawApp, clip: ClipId, selected: &[NoteId], division: Option<Division>) {
    let Some(division) = division else {
        return;
    };
    let notes = (!selected.is_empty()).then_some(selected);
    app.session.label_edit("Quantize notes", None);
    if let Err(e) = app.session.quantize_notes(clip, notes, division, 1.0) {
        app.toast(format!("{e:#}"));
    }
}

/// The horizontal scroll bar (20.2) on `bar`, under a grid `grid_width` points wide: dragging
/// its thumb scrolls the notes sideways, a click beside it jumps there (centred), as the
/// Multitrack's bar does. The wheel's gestures are unchanged.
fn scroll_bar(
    state: &mut PianoRoll,
    ui: &egui::Ui,
    bar: Rect,
    clip_length: f64,
    song_end: f64,
    grid_width: f32,
) {
    let visible = f64::from(grid_width.max(1.0)) * state.ticks_per_px;
    let span = ScrollSpan::new(
        clip_length,
        song_end,
        state.scroll_tick,
        visible,
        state.ticks_per_px,
    );
    let inner = bar.shrink2(egui::vec2(2.0, 2.0));
    let response = ui
        .interact(
            bar,
            ui.scope_id().with("piano-scroll"),
            Sense::click_and_drag(),
        )
        .on_hover_text("Drag to scroll the notes sideways (Shift+wheel also scrolls)");
    let (left, width) = span.thumb(state.scroll_tick, visible, inner.width());
    let thumb = Rect::from_min_size(
        pos2(inner.left() + left, inner.top()),
        egui::vec2(width, inner.height()),
    );
    if response.dragged() {
        let dx = response.drag_delta().x;
        state.scroll_tick = span.dragged(state.scroll_tick, visible, dx, inner.width());
    } else if response.clicked() {
        if let Some(pos) = response
            .interact_pointer_pos()
            .filter(|p| !thumb.contains(*p))
        {
            state.scroll_tick = span.centred(pos.x - inner.left(), visible, inner.width());
        }
    }
    // Drawn where the view now is.
    let (left, width) = span.thumb(state.scroll_tick, visible, inner.width());
    let thumb = Rect::from_min_size(
        pos2(inner.left() + left, inner.top()),
        egui::vec2(width, inner.height()),
    );
    let painter = ui.painter_at(bar);
    painter.rect_filled(bar, 0, theme::PANEL);
    painter.rect_filled(inner, 4, theme::BG);
    let active = response.hovered() || response.dragged();
    let colour = if active {
        theme::TEXT_DIM
    } else {
        theme::GRID_BAR
    };
    painter.rect_filled(thumb, 4, colour);
}

/// The wheel as in the arrangement: plain scrolls the rows, Ctrl zooms time around the
/// pointer, Shift (or a sideways swipe) scrolls time.
fn wheel(state: &mut PianoRoll, ui: &egui::Ui, grid: Rect) {
    if !state.hovered {
        return;
    }
    let (scroll, zoom, pointer) =
        ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
    // Scrolled back no further than the margin before the clip's start.
    if zoom != 1.0 {
        let x = pointer.map_or(0.0, |p| f64::from(p.x - grid.left()).max(0.0));
        let anchor = state.scroll_tick + x * state.ticks_per_px;
        state.ticks_per_px = (state.ticks_per_px / f64::from(zoom)).clamp(0.5, 200.0);
        state.scroll_tick = (anchor - x * state.ticks_per_px).max(earliest(state.ticks_per_px));
    } else {
        state.top_pitch = (state.top_pitch + scroll.y / ROW).clamp(12.0, 127.0);
        state.scroll_tick = (state.scroll_tick - f64::from(scroll.x) * state.ticks_per_px)
            .max(earliest(state.ticks_per_px));
    }
    ui.ctx()
        .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
}

/// The song's bars across the view (0-based), with the tick each starts at.
fn bars(map: &TempoMap, first: u64, last: u64) -> Vec<(i64, u64)> {
    let mut out = Vec::new();
    let mut bar = map.bar_of_tick(first);
    loop {
        let at = map.tick_of_bar(bar, 0);
        if at > last {
            return out;
        }
        out.push((bar, at));
        bar += 1;
    }
}

/// The ruler over the grid, as in Waveform: bar numbers, beats (numbered bar.beat when there
/// is room) and the song grid's division between them.
fn draw_ruler(
    ui: &egui::Ui,
    map: &TempoMap,
    clip: &MidiClip,
    view: &View,
    ruler: Rect,
    grid: Grid,
) {
    let painter = ui.painter_at(ruler);
    painter.rect_filled(ruler, 0, theme::PANEL);
    painter.hline(
        ruler.x_range(),
        ruler.bottom() - 0.5,
        Stroke::new(1.0, theme::GRID_BAR),
    );
    let (first, last) = view.song_ticks(clip);
    let tick_mark = |tick: u64, height: f32, color: Color32| {
        let x = view.song_x(clip, tick);
        painter.vline(
            x,
            ruler.bottom() - height..=ruler.bottom(),
            Stroke::new(1.0, color),
        );
    };
    for line in map.grid_lines(first, last, grid, &view.x_of_tick()) {
        if line.kind == LineKind::Division {
            tick_mark(line.tick, 4.0, theme::TEXT_DIM.gamma_multiply(0.6));
        }
    }
    for (bar, at) in bars(map, first, last) {
        let next = map.tick_of_bar(bar + 1, 0);
        let beat_ticks = map.tick_of_bar(bar, 1).saturating_sub(at).max(1);
        let beat_px = beat_ticks as f64 / view.ticks_per_px;
        for (k, tick) in lines(map, at, next - 1, Division::Beat)
            .into_iter()
            .enumerate()
        {
            if k == 0 {
                tick_mark(tick, RULER, theme::TEXT_DIM);
                painter.text(
                    pos2(view.song_x(clip, tick) + 3.0, ruler.top() + 1.0),
                    Align2::LEFT_TOP,
                    format!("{}", bar + 1),
                    FontId::proportional(11.0),
                    Color32::from_gray(0xd0),
                );
            } else {
                tick_mark(tick, 8.0, theme::TEXT_DIM);
                if beat_px >= 40.0 {
                    painter.text(
                        pos2(view.song_x(clip, tick) + 3.0, ruler.top() + 3.0),
                        Align2::LEFT_TOP,
                        format!("{}.{}", bar + 1, k + 1),
                        FontId::proportional(9.0),
                        theme::TEXT_DIM,
                    );
                }
            }
        }
    }
}

fn draw_grid(
    ui: &egui::Ui,
    map: &TempoMap,
    clip: &MidiClip,
    view: &View,
    song_grid: Grid,
    hover: Option<u8>,
) {
    let painter = ui.painter_at(view.grid);
    let g = view.grid;
    painter.rect_filled(g, 0, theme::BG);
    // Rows: black keys darker, C lines brighter, the row under the pointer lit.
    let bottom = view.pitch(g.bottom());
    let top = view.pitch(g.top());
    for pitch in bottom..=top {
        let y = view.y(pitch);
        let row = Rect::from_min_max(pos2(g.left(), y), pos2(g.right(), y + ROW));
        let fill = if is_black(pitch) {
            theme::LANE
        } else {
            theme::LANE_ALT
        };
        painter.rect_filled(row, 0, fill);
        if hover == Some(pitch) {
            painter.rect_filled(row, 0, Color32::from_white_alpha(14));
        }
        if pitch % 12 == 0 {
            painter.hline(g.x_range(), y + ROW, Stroke::new(1.0, theme::GRID_BAR));
        }
    }
    // Before the clip's start and past its end: dimmed, since notes there don't play.
    let dim = Color32::from_black_alpha(110);
    let start_x = view.x(0.0);
    if start_x > g.left() {
        painter.rect_filled(
            Rect::from_min_max(g.min, pos2(start_x.min(g.right()), g.bottom())),
            0,
            dim,
        );
    }
    let end_x = view.x(clip.length as f64);
    if end_x < g.right() {
        painter.rect_filled(
            Rect::from_min_max(pos2(end_x.max(g.left()), g.top()), g.max),
            0,
            dim,
        );
    }
    // The song's grid (17.4), as the Multitrack draws it: its division where the lines can
    // be told apart, then beats and bars, in the song's bars.
    let (first, last) = view.song_ticks(clip);
    for line in map.grid_lines(first, last, song_grid, &view.x_of_tick()) {
        let x = view.song_x(clip, line.tick);
        let width = if line.kind == LineKind::Bar { 1.5 } else { 1.0 };
        let color = match line.kind {
            // Brighter than the Multitrack's: the rows under them are lighter.
            LineKind::Division => theme::GRID_BEAT.gamma_multiply(0.6),
            kind => crate::arrange::line_color(kind),
        };
        painter.vline(x, g.y_range(), Stroke::new(width, color));
    }
}

fn draw_notes(
    ui: &egui::Ui,
    clip: &MidiClip,
    view: &View,
    selected: &BTreeSet<NoteId>,
    color: Color32,
) {
    let painter = ui.painter_at(view.grid);
    for note in &clip.notes {
        let r = view.note_rect(note);
        if !r.intersects(view.grid) {
            continue;
        }
        let silent = note.tick >= clip.length;
        let strength = 0.45 + 0.55 * f32::from(note.velocity) / 127.0;
        let fill = if silent {
            theme::TEXT_DIM.gamma_multiply(0.4)
        } else {
            color.gamma_multiply(strength)
        };
        painter.rect_filled(r, 2, fill);
        // Its name inside it when there is room: "C#4", else "C#", else none.
        let room = r.width() - 6.0;
        let fits = |text: String| {
            let galley = painter.layout_no_wrap(
                text,
                FontId::proportional(9.0),
                Color32::from_black_alpha(220),
            );
            (galley.size().x <= room).then_some(galley)
        };
        if let Some(galley) =
            fits(pitch_name(note.pitch)).or_else(|| fits(pitch_class(note.pitch).to_string()))
        {
            let at = pos2(r.left() + 3.0, r.center().y - galley.size().y / 2.0);
            painter
                .with_clip_rect(r.intersect(view.grid))
                .galley(at, galley, Color32::BLACK);
        }
        let is_selected = selected.contains(&note.id);
        painter.rect_stroke(
            r,
            2,
            Stroke::new(
                if is_selected { 1.5 } else { 1.0 },
                if is_selected {
                    Color32::WHITE
                } else {
                    Color32::from_black_alpha(160)
                },
            ),
            StrokeKind::Inside,
        );
    }
}

/// The keyboard down the left, drawn as a piano: white keys reaching halfway into the black
/// keys' rows, a line between each pair, names on the Cs, the key under the pointer lit. Click
/// a key to hear it.
fn keyboard(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    track: TrackId,
    keys: Rect,
    view: &View,
    hover: Option<u8>,
) {
    let painter = ui.painter_at(keys);
    let response = ui.interact(
        keys,
        ui.scope_id().with("piano-keys"),
        Sense::click_and_drag(),
    );
    painter.rect_filled(keys, 0, Color32::from_gray(0x30));
    let held = app.piano_roll.as_ref().and_then(|s| s.held_key);
    let lit = |pitch: u8, idle: Color32| {
        if held == Some(pitch) {
            theme::ACCENT
        } else if hover == Some(pitch) {
            idle.lerp_to_gamma(theme::ACCENT, 0.45)
        } else {
            idle
        }
    };
    let (low, high) = (view.pitch(keys.bottom()), view.pitch(keys.top()));
    for pitch in (low..=high).filter(|p| !is_black(*p)) {
        let y = view.y(pitch);
        let above = if pitch < 127 && is_black(pitch + 1) {
            ROW / 2.0
        } else {
            0.0
        };
        let below = if pitch > 0 && is_black(pitch - 1) {
            ROW / 2.0
        } else {
            0.0
        };
        let key = Rect::from_min_max(
            pos2(keys.left(), y - above),
            pos2(keys.right() - 1.0, y + ROW + below),
        );
        painter.rect_filled(key, 0, lit(pitch, Color32::from_gray(0xe6)));
        // The line between this key and the one below.
        painter.hline(
            key.x_range(),
            key.bottom() - 0.5,
            Stroke::new(1.0, Color32::from_gray(0x8a)),
        );
        if pitch % 12 == 0 {
            painter.text(
                pos2(keys.right() - 4.0, y + ROW / 2.0),
                Align2::RIGHT_CENTER,
                pitch_name(pitch),
                FontId::proportional(9.0),
                Color32::from_gray(0x40),
            );
        }
    }
    for pitch in (low..=high).filter(|p| is_black(*p)) {
        let y = view.y(pitch);
        let key = Rect::from_min_max(
            pos2(keys.left(), y + 0.5),
            pos2(keys.left() + KEYS_WIDTH * BLACK_KEY, y + ROW - 0.5),
        );
        painter.rect(
            key,
            1,
            lit(pitch, Color32::from_gray(0x16)),
            Stroke::new(1.0, Color32::BLACK),
            StrokeKind::Inside,
        );
    }
    // Held down: sound the key under the pointer, gliding to the next as it moves.
    let under = response
        .interact_pointer_pos()
        .filter(|_| response.is_pointer_button_down_on())
        .map(|p| key_at(view, keys, p));
    let held = app.piano_roll.as_ref().and_then(|s| s.held_key);
    if under != held {
        if let Some(old) = held {
            app.session.preview_note_off(track, old);
        }
        if let Some(new) = under {
            if let Err(e) = app.session.preview_note(track, new, 100, None) {
                app.toast(format!("{e:#}"));
            }
        }
        if let Some(state) = app.piano_roll.as_mut() {
            state.held_key = under;
        }
    }
}

/// The note a drag holds sounding (20.3), on the track's instrument (as the keyboard's keys and
/// `preview_note` play it): `want` is the pitch and velocity it should sound now, or none.
/// Nothing while recording, nor on a track without an instrument to play it.
fn preview(app: &mut DawApp, track: TrackId, want: Option<(u8, u8)>) {
    let playable = !app.session.is_recording()
        && app.session.project.track(track).is_some_and(|t| {
            t.kind == daw_core::project::TrackKind::Instrument && !t.inserts.is_empty()
        });
    let Some(state) = app.piano_roll.as_mut() else {
        return;
    };
    let sound = state.preview.follow(track, want.filter(|_| playable));
    if let Some((track, pitch)) = sound.off {
        app.session.preview_note_off(track, pitch);
    }
    if let Some((track, pitch, velocity)) = sound.on {
        // Refused only for a track that can't play it, checked above.
        let _ = app
            .session
            .preview_note(track, pitch, velocity.max(1), None);
    }
}

/// A short taste of a note just drawn or moved.
fn taste(app: &mut DawApp, track: TrackId, note: &Note) {
    let _ = app.session.preview_note(
        track,
        note.pitch,
        note.velocity,
        Some(std::time::Duration::from_millis(250)),
    );
}

fn grid_input(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    map: &TempoMap,
    clip: &MidiClip,
    track: TrackId,
    view: &View,
) {
    let response = ui.interact(
        view.grid,
        ui.scope_id().with("piano-grid"),
        Sense::click_and_drag(),
    );
    let (shift, origin) = ui.input(|i| (i.modifiers.shift, i.pointer.press_origin()));
    let Some(state) = app.piano_roll.as_mut() else {
        return;
    };
    let snap = snap_now(ui, state);
    let pos = response.interact_pointer_pos().or(response.hover_pos());
    let hit = pos.and_then(|p| note_at(&clip.notes, view, p));
    if matches!(hit, Some((_, Some(_)))) || matches!(state.drag, Some(Drag::Resize { .. })) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    // Erase.
    if response.secondary_clicked() {
        if let Some((note, _)) = hit {
            let doomed: Vec<NoteId> = if state.selected.contains(&note.id) {
                state.selected.iter().copied().collect()
            } else {
                vec![note.id]
            };
            delete(app, clip.id, &doomed);
        }
        return;
    }
    // Draw.
    if response.double_clicked() && hit.is_none() {
        if let Some(p) = pos {
            let tick = snap_rel(map, clip.start, view.tick(p.x), snap, Rounding::Previous);
            let length = step_at(map, clip.start, tick, snap);
            let note = Note {
                id: 0,
                tick,
                length,
                pitch: view.pitch(p.y),
                velocity: 100,
                channel: 0,
            };
            app.session.label_edit("Add note", None);
            match app.session.project.add_notes(clip.id, &[note]) {
                Ok(ids) => {
                    if let Some(state) = app.piano_roll.as_mut() {
                        state.selected = ids.into_iter().collect();
                    }
                    app.session.fit_midi_clip(clip.id);
                    app.session.mark_dirty();
                    taste(app, track, &note);
                }
                Err(e) => app.toast(e),
            }
        }
        return;
    }
    // Select, and place the playhead (26.4): a click on a note or on empty grid puts it there,
    // as the ruler does. A double-click's first click comes through here (its second draws,
    // above), so it places the playhead too.
    if response.clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            locate_click(app, map, clip.start, view, p.x, snap);
        }
        let Some(state) = app.piano_roll.as_mut() else {
            return;
        };
        match hit {
            Some((note, _)) => {
                if shift {
                    if !state.selected.remove(&note.id) {
                        state.selected.insert(note.id);
                    }
                } else {
                    state.selected = [note.id].into();
                }
                taste(app, track, &note);
            }
            None if !shift => state.selected.clear(),
            None => {}
        }
        return;
    }
    // What the drag's note should sound this frame (20.3), if it changes: struck as a drag
    // takes a note, following its pitch, released when the drag ends.
    let mut sound: Option<Option<(u8, u8)>> = None;
    // Begin a drag, from where the button went down: by the time the pointer has moved far
    // enough to count as a drag it may have left the note's edge (or the note).
    if response.drag_started() {
        state.gesture += 1;
        let from = origin.or(pos).unwrap_or_default();
        state.drag = Some(match note_at(&clip.notes, view, from) {
            Some((note, edge)) => {
                if !state.selected.contains(&note.id) {
                    if !shift {
                        state.selected.clear();
                    }
                    state.selected.insert(note.id);
                }
                let original = clip.notes.clone();
                sound = Some(Some((note.pitch, note.velocity)));
                match edge {
                    Some(edge) => Drag::Resize {
                        original,
                        grab: note,
                        from,
                        edge,
                    },
                    None => Drag::Move {
                        original,
                        grab: note,
                        from,
                    },
                }
            }
            None => Drag::Select { from },
        });
    }
    let gesture = state.gesture;
    let key = format!("piano-drag:{}:{gesture}", clip.id);
    match (&state.drag, pos) {
        (
            Some(Drag::Move {
                original,
                grab,
                from,
            }),
            Some(p),
        ) if response.dragged() => {
            let raw = grab.tick as f64 + f64::from(p.x - from.x) * view.ticks_per_px;
            let to = snap_rel(map, clip.start, raw, snap, Rounding::Nearest);
            let rows = i32::from(view.pitch(p.y)) - i32::from(view.pitch(from.y));
            let notes = moved(original, &state.selected, grab, to, rows);
            // The grabbed note where it is now: heard again only at a new pitch.
            sound = notes
                .iter()
                .find(|n| n.id == grab.id)
                .map(|n| Some((n.pitch, n.velocity)));
            app.session.label_edit("Move notes", Some(key));
            let _ = app.session.project.set_notes(clip.id, notes);
        }
        (
            Some(Drag::Resize {
                original,
                grab,
                from,
                edge,
            }),
            Some(p),
        ) if response.dragged() => {
            // The grabbed note's edge to the pointer (snapped); the others' by as much.
            let dx = f64::from(p.x - from.x) * view.ticks_per_px;
            let was = match edge {
                NoteEdge::Start => grab.tick,
                NoteEdge::End => grab.tick + grab.length,
            };
            let to = snap_rel(map, clip.start, was as f64 + dx, snap, Rounding::Nearest);
            let min = if snap.is_some() {
                step_at(map, clip.start, grab.tick, snap)
            } else {
                1
            };
            let by = to as i64 - was as i64;
            let notes =
                daw_core::midi_clip::resize_notes(original, &state.selected, *edge, by, min);
            app.session.label_edit("Resize notes", Some(key));
            let _ = app.session.project.set_notes(clip.id, notes);
        }
        (Some(Drag::Select { from }), Some(p)) => {
            let band = Rect::from_two_pos(*from, p);
            ui.painter_at(view.grid).rect(
                band,
                0,
                theme::ACCENT.gamma_multiply(0.12),
                Stroke::new(1.0, theme::ACCENT),
                StrokeKind::Inside,
            );
            if response.drag_stopped() {
                let inside: BTreeSet<NoteId> = clip
                    .notes
                    .iter()
                    .filter(|n| view.note_rect(n).intersects(band))
                    .map(|n| n.id)
                    .collect();
                if shift {
                    state.selected.extend(inside);
                } else {
                    state.selected = inside;
                }
            }
        }
        _ => {}
    }
    if response.drag_stopped() {
        let moved_or_sized = matches!(state.drag, Some(Drag::Move { .. } | Drag::Resize { .. }));
        state.drag = None;
        if moved_or_sized {
            app.session.fit_midi_clip(clip.id);
            app.session.mark_dirty();
        }
    }
    // No drag (it ended, or was never one of a note's): nothing held sounding.
    let dragging = app
        .piano_roll
        .as_ref()
        .is_some_and(|s| matches!(s.drag, Some(Drag::Move { .. } | Drag::Resize { .. })));
    if !dragging {
        sound = Some(None);
    }
    if let Some(want) = sound {
        preview(app, track, want);
    }
}

fn delete(app: &mut DawApp, clip: ClipId, notes: &[NoteId]) {
    app.session.label_edit("Delete notes", None);
    if let Err(e) = app.session.project.remove_notes(clip, notes) {
        app.toast(e);
        return;
    }
    if let Some(state) = app.piano_roll.as_mut() {
        for id in notes {
            state.selected.remove(id);
        }
    }
    app.session.mark_dirty();
}

/// Velocity bars under the grid: drag one up or down (all the selected ones, if it is one).
fn velocity_lane(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    clip: &MidiClip,
    view: &View,
    lane: Rect,
    color: Color32,
) {
    let painter = ui.painter_at(lane);
    painter.rect_filled(lane, 0, theme::PANEL);
    painter.hline(
        lane.x_range(),
        lane.top(),
        Stroke::new(1.0, theme::GRID_BAR),
    );
    let Some(state) = app.piano_roll.as_mut() else {
        return;
    };
    let height = lane.height() - 6.0;
    let y_of = |velocity: u8| lane.bottom() - 3.0 - height * f32::from(velocity) / 127.0;
    for note in &clip.notes {
        let x = view.x(note.tick as f64);
        if x < lane.left() || x > lane.right() {
            continue;
        }
        let selected = state.selected.contains(&note.id);
        let stroke = Stroke::new(
            if selected { 3.0 } else { 2.0 },
            if selected { Color32::WHITE } else { color },
        );
        painter.vline(x + 1.0, y_of(note.velocity)..=lane.bottom() - 3.0, stroke);
        painter.circle_filled(pos2(x + 1.0, y_of(note.velocity)), 2.5, stroke.color);
    }
    let response = ui.interact(lane, ui.scope_id().with("piano-velocity"), Sense::drag());
    let pos = response.interact_pointer_pos();
    if response.drag_started() {
        // The note nearest the pointer in time, a selected one if it's within reach.
        let near = pos.and_then(|p| {
            clip.notes
                .iter()
                .map(|n| (n, (view.x(n.tick as f64) - p.x).abs()))
                .filter(|(_, d)| *d < 12.0)
                .min_by(|a, b| {
                    let rank = |n: &Note, d: f32| (!state.selected.contains(&n.id), d);
                    rank(a.0, a.1)
                        .partial_cmp(&rank(b.0, b.1))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(n, _)| n.id)
        });
        state.gesture += 1;
        state.drag = near.map(|grab| Drag::Velocity {
            original: clip.notes.clone(),
            grab,
        });
    }
    if let (Some(Drag::Velocity { original, grab }), Some(p)) = (&state.drag, pos) {
        let velocity = ((lane.bottom() - 3.0 - p.y) / height * 127.0)
            .round()
            .clamp(1.0, 127.0) as u8;
        let group = state.selected.contains(grab);
        let notes: Vec<Note> = original
            .iter()
            .map(|n| {
                if n.id == *grab || (group && state.selected.contains(&n.id)) {
                    Note { velocity, ..*n }
                } else {
                    *n
                }
            })
            .collect();
        let key = format!("piano-velocity:{}:{}", clip.id, state.gesture);
        app.session.label_edit("Note velocity", Some(key));
        let _ = app.session.project.set_notes(clip.id, notes);
        if response.drag_stopped() {
            if let Some(state) = app.piano_roll.as_mut() {
                state.drag = None;
            }
            app.session.mark_dirty();
        }
    }
}

/// The editor's keys, while the pointer is over it: Delete erases the selected notes, Ctrl+A
/// selects every note, Q quantizes, Escape clears the selection. Returns whether it took the
/// keys (so the arrangement's Delete doesn't remove the clip).
pub fn shortcuts(app: &mut DawApp, ctx: &egui::Context) -> bool {
    use egui::{Key, Modifiers};
    let Some(state) = app.piano_roll.as_ref().filter(|s| s.hovered) else {
        return false;
    };
    let clip = state.clip;
    let selected: Vec<NoteId> = state.selected.iter().copied().collect();
    let division = state.division();
    let pressed = |key: Key, m: Modifiers| ctx.input_mut(|i| i.consume_key(m, key));
    // Consumed even with nothing selected, so the arrangement doesn't delete the clip.
    let erase = pressed(Key::Delete, Modifiers::NONE) || pressed(Key::Backspace, Modifiers::NONE);
    if erase && !selected.is_empty() {
        delete(app, clip, &selected);
    }
    if pressed(Key::A, Modifiers::COMMAND) {
        let all = app
            .session
            .project
            .find_midi_clip(clip)
            .map(|(_, c)| c.notes.iter().map(|n| n.id).collect())
            .unwrap_or_default();
        if let Some(state) = app.piano_roll.as_mut() {
            state.selected = all;
        }
    }
    if pressed(Key::Q, Modifiers::NONE) {
        quantize(app, clip, &selected, division);
    }
    if pressed(Key::Escape, Modifiers::NONE) {
        if let Some(state) = app.piano_roll.as_mut() {
            state.selected.clear();
        }
    }
    true
}

/// Clicks the arrangement's clip menu and keys reach the editor through.
pub fn open(app: &mut DawApp, clip: ClipId) -> Result<(), String> {
    let Some((_, c)) = app.session.project.find_midi_clip(clip) else {
        return Err(format!("no MIDI clip with id {clip}"));
    };
    if app.piano_roll.as_ref().map(|p| p.clip) != Some(clip) {
        app.piano_roll = Some(PianoRoll::open(c));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Notes heard as they are dragged (20.3): struck when the drag takes the note, not again
    /// while it moves along in time, once for each pitch it is dragged to (the last released
    /// first), and released when the drag ends; another track's note is a new note too.
    #[test]
    fn a_dragged_note_sounds_once_per_pitch_and_stops_on_release() {
        let mut preview = NotePreview::default();
        let none = PreviewSound::default();
        // Nothing dragged: nothing sounds or stops.
        assert_eq!(preview.follow(1, None), none);
        // The drag takes a C4 at velocity 90.
        assert_eq!(
            preview.follow(1, Some((60, 90))),
            PreviewSound {
                off: None,
                on: Some((1, 60, 90))
            }
        );
        // Moved along in time: the same pitch, nothing new.
        for _ in 0..5 {
            assert_eq!(preview.follow(1, Some((60, 90))), none);
        }
        // Up a tone, and on up: the old note released, the new one struck, once each.
        assert_eq!(
            preview.follow(1, Some((62, 90))),
            PreviewSound {
                off: Some((1, 60)),
                on: Some((1, 62, 90))
            }
        );
        assert_eq!(preview.follow(1, Some((62, 90))), none);
        assert_eq!(
            preview.follow(1, Some((64, 90))),
            PreviewSound {
                off: Some((1, 62)),
                on: Some((1, 64, 90))
            }
        );
        // Let go: released.
        assert_eq!(
            preview.follow(1, None),
            PreviewSound {
                off: Some((1, 64)),
                on: None
            }
        );
        assert_eq!(preview.follow(1, None), none);
        // The next drag, on another track's clip, at the same pitch: struck there.
        assert_eq!(preview.follow(1, Some((64, 100))).on, Some((1, 64, 100)));
        assert_eq!(
            preview.follow(2, Some((64, 100))),
            PreviewSound {
                off: Some((1, 64)),
                on: Some((2, 64, 100))
            }
        );
        assert_eq!(preview.release(), Some((2, 64)));
        assert_eq!(preview.release(), None);
    }

    /// The scroll bar (20.2): it spans from the margin before the clip to a quarter view past
    /// the later of the clip's and the song's ends (or the view's end); the thumb is the view's
    /// share of it, where the view is; dragging it a point moves the view the span's ticks per
    /// point, never before the margin nor on past the end; a click beside it centres the view
    /// there.
    #[test]
    fn the_scroll_bar_shows_and_drags_the_view() {
        let tpp = 2.0;
        let visible = 1000.0 * tpp; // a grid 1000 points wide
                                    // A clip of 7680 ticks (two bars of 4/4), the song ending earlier: the clip decides.
        let span = ScrollSpan::new(7680.0, 3000.0, 0.0, visible, tpp);
        assert_eq!(span.first, -f64::from(MARGIN) * tpp);
        assert_eq!(span.last, 7680.0 + visible / 4.0);
        let ticks = span.last - span.first;
        // A bar 500 points wide: the thumb is the view's share, at the view's start.
        let (left, width) = span.thumb(0.0, visible, 500.0);
        assert!(
            (width - (visible / ticks * 500.0) as f32).abs() < 1e-3,
            "{width}"
        );
        assert!(
            (left - (-span.first / ticks * 500.0) as f32).abs() < 1e-3,
            "{left}"
        );
        // Dragged 100 points: the view moves 100 points' worth of the span's ticks.
        let moved = span.dragged(0.0, visible, 100.0, 500.0);
        assert!((moved - 100.0 * ticks / 500.0).abs() < 1e-9, "{moved}");
        let (after, _) = span.thumb(moved, visible, 500.0);
        assert!(
            (after - left - 100.0).abs() < 1e-3,
            "the thumb follows the pointer"
        );
        // Never before the margin, nor on past the end.
        assert_eq!(span.dragged(0.0, visible, -1000.0, 500.0), span.first);
        assert_eq!(
            span.dragged(0.0, visible, 5000.0, 500.0),
            span.last - visible
        );
        // A click beside the thumb: the view centred there.
        let centred = span.centred(250.0, visible, 500.0);
        assert!((centred + visible / 2.0 - (span.first + ticks / 2.0)).abs() < 1e-9);
        // The song longer than the clip: the bar spans the song too.
        let song = ScrollSpan::new(7680.0, 40_000.0, 0.0, visible, tpp);
        assert_eq!(song.last, 40_000.0 + visible / 4.0);
        // The view past every end (zoomed out): the bar spans the view, its thumb whole.
        let wide = ScrollSpan::new(7680.0, 3000.0, 0.0, 100_000.0, tpp);
        assert_eq!(wide.last, 100_000.0);
        let (left, width) = wide.thumb(0.0, 100_000.0, 500.0);
        assert!(left >= 0.0 && left + width <= 500.0 + 1e-3);
        // A tiny view of a long song: the thumb keeps a size to grab.
        let long = ScrollSpan::new(7680.0, 10_000_000.0, 0.0, visible, tpp);
        assert_eq!(long.thumb(0.0, visible, 500.0).1, MIN_THUMB);
    }

    /// The ruler (16.3): a click puts the playhead on the song sample under it, snapped to the
    /// grid in the song's bars (the clip starts mid-bar) or to the tick, through a tempo change,
    /// and never before the song's start.
    #[test]
    fn the_ruler_puts_the_playhead_on_the_song_sample_under_it() {
        // 120 BPM 4/4 at 48 kHz (a quarter is 24000 samples), 60 BPM from bar 3 (tick 7680).
        let mut project = daw_core::project::Project::default();
        project.tempo_changes.push(daw_core::project::TempoChange {
            bar: 2,
            beat: 0,
            tempo: Some(60.0),
            time_signature: None,
        });
        let map = TempoMap::of(&project, 48_000);
        // The clip starts at bar 1 beat 3 (tick 1920); 10 ticks a point from its start.
        let view = View {
            grid: Rect::from_min_size(pos2(100.0, 0.0), egui::vec2(800.0, 300.0)),
            start_tick: 0.0,
            ticks_per_px: 10.0,
            top_pitch: 72.0,
        };
        let at = |x: f32, snap| ruler_sample(&map, 1920, &view, x, snap);
        // 50 points in: tick 2420, a sixteenth (240 ticks) from 2400.
        assert_eq!(at(150.0, Some(SIXTEENTH)), 60_000, "snapped to tick 2400");
        assert_eq!(at(150.0, None), 60_500, "to the tick: 2420");
        // A quarter into the slower bars: 8 quarters at 120, then one at 60.
        let slow = 100.0 + (8640.0 - 1920.0) / 10.0;
        assert_eq!(at(slow, Some(Division::Beat)), 8 * 24_000 + 48_000);
        assert_eq!(at(slow + 1.0, None), 8 * 24_000 + 48_000 + 500);
        // Scrolled to before the song's start: the start.
        let early = View {
            start_tick: -4000.0,
            ..view
        };
        assert_eq!(ruler_sample(&map, 1920, &early, 100.0, None), 0);
    }

    /// 26.4: a click in the note grid (on a note or not: only x counts) puts the playhead where
    /// a click on the ruler above it does: with Snap on on the grid line nearest, off on the
    /// tick under it, through a tempo change; a press beyond the grid's sides as at its edge.
    #[test]
    fn a_grid_click_puts_the_playhead_where_the_ruler_does() {
        // 120 BPM 4/4 at 48 kHz (a tick is 25 samples), 60 BPM from bar 3 (tick 7680).
        let mut project = daw_core::project::Project::default();
        project.tempo_changes.push(daw_core::project::TempoChange {
            bar: 2,
            beat: 0,
            tempo: Some(60.0),
            time_signature: None,
        });
        let map = TempoMap::of(&project, 48_000);
        // The clip starts at bar 1 beat 3 (tick 1920); 10 ticks a point, the grid 100..900.
        let view = View {
            grid: Rect::from_min_size(pos2(100.0, 0.0), egui::vec2(800.0, 300.0)),
            start_tick: 0.0,
            ticks_per_px: 10.0,
            top_pitch: 72.0,
        };
        let click = |x: f32, snap| clicked_sample(&map, 1920, &view, x, snap);
        // 63 points in: tick 2550, between the sixteenths at 2400 and 2640.
        assert_eq!(
            click(163.0, Some(SIXTEENTH)),
            2_640 * 25,
            "Snap on: the nearer line"
        );
        assert_eq!(
            click(163.0, None),
            2_550 * 25,
            "Snap off: the tick under it"
        );
        // A double-click's note there starts on the line before it (it draws from the
        // pointer's step); the playhead goes to the nearer line, as the ruler's click.
        let note = snap_rel(
            &map,
            1920,
            view.tick(163.0),
            Some(SIXTEENTH),
            Rounding::Previous,
        );
        assert_eq!(1920 + note, 2_400);
        // Past the tempo change: a quarter into the 60 BPM bars, plus a point.
        let slow = 100.0 + (8640.0 - 1920.0) / 10.0;
        assert_eq!(click(slow, Some(Division::Beat)), 8 * 24_000 + 48_000);
        assert_eq!(click(slow + 1.0, None), 8 * 24_000 + 48_000 + 500);
        // The ruler and the grid agree everywhere across the grid's width.
        for x in [100.0, 163.0, 437.5, slow, 899.0] {
            for snap in [Some(SIXTEENTH), Some(Division::Beat), None] {
                assert_eq!(click(x, snap), ruler_sample(&map, 1920, &view, x, snap));
            }
        }
        // Beyond the sides: as at the edges.
        assert_eq!(click(40.0, None), click(100.0, None));
        assert_eq!(click(2_000.0, None), click(899.0, None));
    }

    /// 26.3: a dragged playhead stays in the grid's width: past an edge, on the line inside
    /// it when the nearest is past it, and at the edge itself zoomed in past the grid.
    #[test]
    fn a_dragged_playhead_stays_in_the_piano_rolls_view() {
        // 120 BPM 4/4 at 48 kHz: a tick is 25 samples, a sixteenth 240 ticks.
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        let view = |start_tick: f64, ticks_per_px: f64| View {
            grid: Rect::from_min_size(pos2(100.0, 0.0), egui::vec2(800.0, 300.0)),
            start_tick,
            ticks_per_px,
            top_pitch: 72.0,
        };
        let x_of = |v: &View, sample: u64| v.x(map.tick_at(sample as f64));
        // 12 ticks a point (a sixteenth 20 points): far past the right edge the point is 899
        // (tick 212 + 9588 = 9800): its nearest line, 9840, is past the edge (902.3); the line
        // inside, 9600, is 17 points in.
        let near = view(212.0, 12.0);
        let at = drag_sample(&map, 0, &near, 2000.0, Some(SIXTEENTH));
        assert_eq!(at, 9_600 * 25);
        assert!((100.0..=900.0).contains(&x_of(&near, at)));
        // Far past the left: the point at 100 (tick 2410): the nearest line, 2400, is past the
        // edge; the line inside, 2640, is 19 points in.
        let back = view(2_410.0, 12.0);
        assert_eq!(
            drag_sample(&map, 0, &back, -500.0, Some(SIXTEENTH)),
            2_640 * 25
        );
        // Half a tick a point: a sixteenth is 480 points, no line near the edge: the edge.
        let deep = view(1_000.0, 0.5);
        let at = drag_sample(&map, 0, &deep, 2000.0, Some(SIXTEENTH));
        assert_eq!(at, ruler_sample(&map, 0, &deep, 899.0, None));
        assert!((100.0..=900.0).contains(&x_of(&deep, at)));
    }
    use egui::vec2;

    fn note(id: NoteId, tick: u64, length: u64, pitch: u8) -> Note {
        Note {
            id,
            tick,
            length,
            pitch,
            velocity: 100,
            channel: 0,
        }
    }

    #[test]
    fn snapping_follows_the_songs_bars_not_the_clips_start() {
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        let sixteenth = "1/16".parse().ok();
        // A clip starting 100 ticks into a bar: its grid lines sit at 140, 380... of its own.
        assert_eq!(
            snap_rel(&map, 100, 130.0, sixteenth, Rounding::Nearest),
            140
        );
        assert_eq!(snap_rel(&map, 100, 130.0, None, Rounding::Nearest), 130);
        assert_eq!(snap_rel(&map, 0, -50.0, sixteenth, Rounding::Nearest), 0);
        assert_eq!(step_at(&map, 0, 500, sixteenth), 240);
        assert_eq!(step_at(&map, 0, 500, Some(Division::Beat)), 960);
    }

    #[test]
    fn moving_keeps_the_selection_together_inside_the_clip_and_keyboard() {
        let notes = vec![
            note(1, 480, 240, 60),
            note(2, 960, 240, 64),
            note(3, 0, 100, 70),
        ];
        let both: BTreeSet<NoteId> = [1, 2].into();
        let moved_by = |to, rows| -> Vec<(u64, u8)> {
            moved(&notes, &both, &notes[1], to, rows)
                .iter()
                .map(|n| (n.tick, n.pitch))
                .collect()
        };
        // Grab note 2 and drop it at 1440, two rows up: note 1 follows; note 3 stays.
        assert_eq!(moved_by(1440, 2), [(960, 62), (1440, 66), (0, 70)]);
        // Dragged back past the start: stops where note 1 reaches 0.
        assert_eq!(moved_by(0, 0), [(0, 60), (480, 64), (0, 70)]);
        // And off the top of the keyboard: stops at 127.
        assert_eq!(moved_by(960, 100)[1].1, 127);
    }

    #[test]
    fn edges_grab_either_end_and_white_keys_reach_into_black_rows() {
        let view = View {
            grid: Rect::from_min_size(pos2(100.0, 0.0), vec2(500.0, 240.0)),
            start_tick: 0.0,
            ticks_per_px: 8.0,
            top_pitch: 72.0,
        };
        // 960 ticks long from 960: x 220 to 340, pitch 60's row from y 144.
        let notes = [note(1, 960, 960, 60)];
        let at = |x: f32| note_at(&notes, &view, pos2(x, 150.0)).map(|(_, e)| e);
        assert_eq!(at(221.0), Some(Some(NoteEdge::Start)));
        assert_eq!(at(280.0), Some(None));
        assert_eq!(at(338.0), Some(Some(NoteEdge::End)));
        assert_eq!(at(350.0), None);
        // A note 6 points wide keeps a middle.
        let short = [note(1, 960, 48, 60)];
        assert_eq!(note_at(&short, &view, pos2(223.0, 150.0)).unwrap().1, None);

        let keys = Rect::from_min_size(pos2(0.0, 0.0), vec2(KEYS_WIDTH, 240.0));
        // C#4 (61) has the row above C4: its black key on the left, C4 and D4 to the right.
        let row = view.y(61);
        assert_eq!(key_at(&view, keys, pos2(10.0, row + 3.0)), 61);
        assert_eq!(key_at(&view, keys, pos2(50.0, row + 3.0)), 62);
        assert_eq!(key_at(&view, keys, pos2(50.0, row + 9.0)), 60);
        assert_eq!(key_at(&view, keys, pos2(50.0, view.y(64) + 3.0)), 64);
    }

    #[test]
    fn names_and_rows() {
        assert_eq!(pitch_name(60), "C4");
        assert_eq!(pitch_name(69), "A4");
        assert!(is_black(61) && !is_black(64));
        let view = View {
            grid: Rect::from_min_size(pos2(0.0, 100.0), vec2(500.0, 240.0)),
            start_tick: 0.0,
            ticks_per_px: 8.0,
            top_pitch: 80.0,
        };
        // Pitch 80's row runs from the top edge down one row.
        assert_eq!(view.y(80), 100.0);
        assert_eq!(view.pitch(100.5), 80);
        assert_eq!(view.pitch(100.0 + ROW + 0.5), 79);
        assert_eq!(view.x(960.0), 120.0);
        assert!((view.tick(120.0) - 960.0).abs() < 1e-9);
    }
}
