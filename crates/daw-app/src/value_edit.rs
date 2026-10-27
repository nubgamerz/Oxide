//! Values typed in place (dev plan 22.1, 22.2): a track's name in its header and the
//! inspector, and the volume and pan readouts beside the header's knobs, in the inspector and
//! on the mixer's strips. Every one of them behaves alike:
//!
//! - opening (a double-click on a header's name, a click on a readout or the inspector's name)
//!   shows a text field with its text selected, so typing replaces it;
//! - Enter keeps what was typed, and so does clicking away (or Tab), as other DAWs do;
//! - Escape keeps the old value;
//! - text that doesn't read (an empty name, "loud" for a volume) is refused: on Enter the field
//!   stays open with a red outline saying why; clicking away drops it and keeps the old value.
//!
//! The field asks for keyboard focus once, when it opens, and closes when it loses it. The
//! header's old rename field asked for focus every frame, right after drawing itself: Enter and
//! a click elsewhere both take focus away inside the frame, the request gave it straight back,
//! so `lost_focus` never fired and the name was never written (22.1's bug,
//! `tests::asking_for_focus_every_frame_never_lets_go`).
//!
//! What a field commits goes through the session (`Session::rename_track`,
//! `Session::set_mix_level`), the same paths `set_track` and `set_track_mixer` take, inside the
//! frame's edit: each commit is one undo step.

use crate::theme;
use daw_core::project::{TrackId, GAIN_RANGE};
use egui::{vec2, FontId, Id, Key, Rect, RichText, Sense, Stroke, Ui, Vec2};
use std::sync::atomic::{AtomicBool, Ordering};

/// How an open field ended this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    Enter,
    Escape,
    /// Focus went elsewhere: a click outside, Tab.
    Away,
}

/// What to do with the text when a field ends.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision<T> {
    Commit(T),
    /// Close and keep the old value.
    Cancel,
    /// Stay open, saying why (Enter on text that doesn't read).
    Refuse(String),
}

/// The rules every field follows: Escape cancels; Enter commits text that reads and refuses
/// text that doesn't; clicking away commits text that reads and drops text that doesn't.
pub fn decide<T>(ending: Ending, parsed: Result<T, String>) -> Decision<T> {
    match (ending, parsed) {
        (Ending::Escape, _) => Decision::Cancel,
        (_, Ok(value)) => Decision::Commit(value),
        (Ending::Enter, Err(why)) => Decision::Refuse(why),
        (Ending::Away, Err(_)) => Decision::Cancel,
    }
}

/// A field's state while it is open, kept in egui's memory under its id.
#[derive(Debug, Clone, Default)]
struct Open {
    text: String,
    /// What the field edits (a track, or [`MASTER`]): a field whose place shows another target
    /// now (the inspector after another track was selected) commits to this one.
    target: u32,
    /// Focus has been asked for (once, on the first frame drawn).
    focused: bool,
    /// Why Enter was refused, shown until the text changes.
    refused: Option<String>,
}

/// The narrowest a volume or pan field is.
const FIELD_WIDTH: f32 = 56.0;

/// The target of a field on the master.
pub const MASTER: u32 = u32::MAX;

/// What the inspector shows when it shows neither a track nor the master.
pub const NOTHING: u32 = u32::MAX - 1;

/// The target of a field on `owner` (a track, or the master).
pub fn target_of(owner: Option<TrackId>) -> u32 {
    owner.unwrap_or(MASTER)
}

/// Open the field `id` on `text` (selected when it is first drawn), editing `target`.
pub fn open(ctx: &egui::Context, id: Id, text: impl Into<String>, target: u32) {
    let open = Open {
        text: text.into(),
        target,
        ..Default::default()
    };
    ctx.data_mut(|d| d.insert_temp(id, open));
}

/// Whether the field `id` is open.
pub fn is_open(ctx: &egui::Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<Open>(id)).is_some()
}

/// The field `id`'s text so far, while open (checks).
pub fn text(ctx: &egui::Context, id: Id) -> Option<String> {
    ctx.data(|d| d.get_temp::<Open>(id)).map(|o| o.text)
}

/// Whether Enter was refused on the field `id`'s text (checks).
pub fn refused(ctx: &egui::Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<Open>(id))
        .is_some_and(|o| o.refused.is_some())
}

/// The field `id` is open on another target than `current` (its place now shows another
/// track): closed, with its target and text, which the caller commits as clicking away would.
pub fn orphaned(ctx: &egui::Context, id: Id, current: u32) -> Option<(u32, String)> {
    let open = ctx.data(|d| d.get_temp::<Open>(id))?;
    if open.target == current {
        return None;
    }
    ctx.data_mut(|d| d.remove_temp::<Open>(id));
    Some((open.target, open.text))
}

/// The open field `id`, `size` big: what it decided this frame (`None` while it stays open, or
/// when it isn't open).
pub fn field<T>(
    ui: &mut Ui,
    id: Id,
    size: Vec2,
    font: FontId,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Option<Decision<T>> {
    let mut open = ui.data(|d| d.get_temp::<Open>(id))?;
    let edit_id = id.with("text");
    let edit = egui::TextEdit::singleline(&mut open.text)
        .id(edit_id)
        .font(font)
        .margin(egui::Margin::symmetric(3, 1))
        // Escape stays with the field (it cancels); egui would otherwise take focus away first,
        // and the app's own Escape (clear the selection) would take the key.
        .event_filter(egui::EventFilter {
            tab: false,
            horizontal_arrows: true,
            vertical_arrows: true,
            escape: true,
        });
    let response = ui.add_sized(size, edit);
    if response.changed() {
        open.refused = None;
    }
    let focused = ui.memory(|m| m.has_focus(edit_id));
    let ending = if !open.focused {
        // Once, as it opens: focus, and the text selected so typing replaces it.
        response.request_focus();
        let mut state = egui::TextEdit::load_state(ui.ctx(), edit_id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::select_all_str(&open.text)));
        state.store(ui.ctx(), edit_id);
        open.focused = true;
        None
    } else if focused && ui.input(|i| i.key_pressed(Key::Escape)) {
        ui.memory_mut(|m| m.surrender_focus(edit_id));
        Some(Ending::Escape)
    } else if response.lost_focus() {
        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        Some(if enter { Ending::Enter } else { Ending::Away })
    } else if !focused {
        // Not drawn for a while (scrolled away) and so not focused: what it held is stale.
        Some(Ending::Escape)
    } else {
        None
    };
    let decision = ending.map(|e| decide(e, parse(&open.text)));
    match &decision {
        Some(Decision::Refuse(why)) => {
            open.refused = Some(why.clone());
            response.request_focus();
        }
        Some(_) => {
            ui.data_mut(|d| d.remove_temp::<Open>(id));
            return decision;
        }
        None => {}
    }
    if let Some(why) = &open.refused {
        ui.painter().rect_stroke(
            response.rect.expand(1.0),
            2,
            Stroke::new(1.5, theme::REFUSED),
            egui::StrokeKind::Outside,
        );
        response.on_hover_text(why.as_str());
    }
    ui.data_mut(|d| d.insert_temp(id, open));
    match decision {
        Some(Decision::Refuse(why)) => Some(Decision::Refuse(why)),
        _ => None,
    }
}

/// Where the field ids' places are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Place {
    Header,
    Inspector,
    Mixer,
}

/// The volume or the pan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    Volume,
    Pan,
}

/// The id of a track's name field. The inspector's has one for every track it shows
/// (`track` `None`): it edits whichever was selected when it opened.
pub fn name_id(place: Place, track: Option<TrackId>) -> Id {
    Id::unique(("track-name-field", place, track))
}

/// The id of a volume or pan readout. The inspector's is one for all (`owner` ignored), as
/// its name's is.
pub fn level_id(place: Place, owner: Option<TrackId>, level: Level) -> Id {
    let owner = (place != Place::Inspector).then_some(target_of(owner));
    Id::unique(("level-field", place, owner, level))
}

/// A volume or pan typed into a readout this frame, applied once the drawing's borrows of
/// the project are over ([`Typed::apply`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Typed {
    pub owner: Option<TrackId>,
    pub gain_db: Option<f32>,
    pub pan: Option<f32>,
}

impl Typed {
    pub fn for_owner(owner: Option<TrackId>) -> Self {
        Self {
            owner,
            ..Default::default()
        }
    }

    /// Through `Session::set_mix_level`, as `set_track_mixer` sets them: one undo step.
    pub fn apply(self, app: &mut crate::app::DawApp) {
        if self.gain_db.is_none() && self.pan.is_none() {
            return;
        }
        if let Err(e) = app
            .session
            .set_mix_level(self.owner, self.gain_db, self.pan)
        {
            app.toast(format!("{e:#}"));
        }
    }
}

/// A volume (`Level::Volume`, dB) or pan (-1 left to 1 right) readout showing `shown`, `size`
/// big; clicked while `editable`, it turns into a field. Returns a value typed and committed
/// this frame.
#[allow(clippy::too_many_arguments)] // each readout's place, look and target, as drawn
pub fn level(
    ui: &mut Ui,
    id: Id,
    level: Level,
    shown: f32,
    size: Vec2,
    font: FontId,
    editable: bool,
    target: u32,
) -> Option<f32> {
    let parse = match level {
        Level::Volume => parse_db,
        Level::Pan => parse_pan,
    };
    if is_open(ui.ctx(), id) {
        if !editable {
            // A lane took the control over (or a write pass began) while it was open.
            ui.data_mut(|d| d.remove_temp::<Open>(id));
        } else {
            // Wider than the number it replaces, so "-6.5 dB" shows whole as it is typed.
            let size = vec2(size.x.max(FIELD_WIDTH), size.y);
            return match field(ui, id, size, font, parse) {
                Some(Decision::Commit(value)) => Some(value),
                _ => None,
            };
        }
    }
    let text = match level {
        Level::Volume => format!("{shown:+.1}"),
        Level::Pan => crate::inspector::pan_label(shown),
    };
    let label = egui::Label::new(
        RichText::new(&text)
            .font(font.clone())
            .color(theme::TEXT_DIM),
    )
    .sense(if editable {
        Sense::click()
    } else {
        Sense::hover()
    });
    let response = ui.add_sized(size, label);
    remember(ui, id, response.rect, &text);
    if !editable {
        return None;
    }
    if response.hovered() {
        // A faint box says it can be typed into.
        ui.painter().rect_stroke(
            response.rect,
            2,
            Stroke::new(1.0, theme::TEXT_DIM.gamma_multiply(0.5)),
            egui::StrokeKind::Inside,
        );
    }
    let what = match level {
        Level::Volume => "a volume in dB (-6, -6.5 dB, +3, -inf)",
        Level::Pan => "a pan (L30, R20, C; -30 is L30)",
    };
    let response = response
        .on_hover_cursor(egui::CursorIcon::Text)
        .on_hover_text(format!("Click to type {what}"));
    if response.clicked() {
        open(ui.ctx(), id, text, target);
    }
    None
}

/// Checks only: where each readout and name was drawn, and what it showed, under its id
/// (`edit_check`). Off unless a check run turns it on.
static REMEMBER: AtomicBool = AtomicBool::new(false);

/// Keep where readouts and names are drawn (a check run).
pub fn remember_places() {
    REMEMBER.store(true, Ordering::Relaxed);
}

/// What a readout or name showed and where, as last drawn.
#[derive(Debug, Clone)]
pub struct Drawn {
    pub rect: Rect,
    pub text: String,
}

/// Note where `id`'s readout or name was drawn and what it showed (a check run).
pub fn remember(ui: &Ui, id: Id, rect: Rect, text: &str) {
    if REMEMBER.load(Ordering::Relaxed) {
        let drawn = Drawn {
            rect,
            text: text.to_string(),
        };
        ui.data_mut(|d| d.insert_temp(id.with("drawn"), drawn));
    }
}

/// Where `id`'s readout or name was last drawn, and what it showed.
pub fn drawn(ctx: &egui::Context, id: Id) -> Option<Drawn> {
    ctx.data(|d| d.get_temp::<Drawn>(id.with("drawn")))
}

/// The text as typed, with a typographic minus or a decimal comma read as "-" and ".".
fn tidy(text: &str) -> String {
    text.trim().replace('\u{2212}', "-").replace(',', ".")
}

/// A volume typed in dB: "-6", "-6.5", "-6.5 dB", "+3", "0", "-inf" or "-∞" (the fader's
/// bottom), clamped to the fader's range. Anything else is refused, saying what reads.
pub fn parse_db(text: &str) -> Result<f32, String> {
    let tidy = tidy(text).to_lowercase();
    let number = tidy.strip_suffix("db").unwrap_or(&tidy).trim();
    let value = match number {
        "-inf" | "-infinity" | "-∞" => f32::NEG_INFINITY,
        _ => number
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| {
                format!(
                    "\"{}\" isn't a volume: type dB, as -6, -6.5 dB, +3 or -inf",
                    text.trim()
                )
            })?,
    };
    Ok(value.clamp(GAIN_RANGE.0, GAIN_RANGE.1))
}

/// A pan typed as the readouts show it: "C" (centre), "L30", "R20" (percent; "30L" too), or a
/// number, -30 being L30 and 30 R30; clamped to L100..R100. Anything else is refused.
pub fn parse_pan(text: &str) -> Result<f32, String> {
    let tidy = tidy(text).to_uppercase();
    let tidy = tidy.trim_end_matches('%').trim();
    let bad = || {
        format!(
            "\"{}\" isn't a pan: type L30, R20, C, or -30 for L30",
            text.trim()
        )
    };
    let amount = |digits: &str| {
        digits
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
    };
    let percent = match tidy {
        "C" | "CENTRE" | "CENTER" => 0.0,
        _ if tidy.starts_with('L') => -amount(&tidy[1..]).ok_or_else(bad)?,
        _ if tidy.starts_with('R') => amount(&tidy[1..]).ok_or_else(bad)?,
        _ if tidy.ends_with('L') => -amount(&tidy[..tidy.len() - 1]).ok_or_else(bad)?,
        _ if tidy.ends_with('R') => amount(&tidy[..tidy.len() - 1]).ok_or_else(bad)?,
        _ => tidy
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(bad)?,
    };
    Ok((percent / 100.0).clamp(-1.0, 1.0))
}

/// A track's name as typed: trimmed, an empty one refused (`project::track_name`).
pub fn parse_name(text: &str) -> Result<String, String> {
    daw_core::project::track_name(text)
        .map(str::to_string)
        .map_err(|_| "A track needs a name: type one, or Escape to keep the old one".to_string())
}

/// A field of `size` for the name of `target`, opened by the caller: a committed name, typed
/// and checked (`parse_name`).
pub fn name_field(ui: &mut Ui, id: Id, size: Vec2, font: FontId) -> Option<String> {
    match field(ui, id, size, font, parse_name) {
        Some(Decision::Commit(name)) => Some(name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput};

    #[test]
    fn the_rules_commit_cancel_and_refuse() {
        let ok = || Ok::<_, String>(-6.0);
        let bad = || Err::<f32, _>("no".to_string());
        assert_eq!(decide(Ending::Enter, ok()), Decision::Commit(-6.0));
        assert_eq!(decide(Ending::Away, ok()), Decision::Commit(-6.0));
        assert_eq!(decide(Ending::Escape, ok()), Decision::Cancel);
        assert_eq!(decide(Ending::Enter, bad()), Decision::Refuse("no".into()));
        assert_eq!(decide(Ending::Away, bad()), Decision::Cancel);
        assert_eq!(decide(Ending::Escape, bad()), Decision::Cancel);
    }

    #[test]
    fn volumes_read_in_every_form() {
        assert_eq!(parse_db("-6"), Ok(-6.0));
        assert_eq!(parse_db("-6.5"), Ok(-6.5));
        assert_eq!(parse_db("-6.5 dB"), Ok(-6.5));
        assert_eq!(parse_db(" -6.5dB "), Ok(-6.5));
        assert_eq!(parse_db("-6.5 DB"), Ok(-6.5));
        assert_eq!(parse_db("+3"), Ok(3.0));
        assert_eq!(parse_db("3"), Ok(3.0));
        assert_eq!(parse_db("0"), Ok(0.0));
        assert_eq!(
            parse_db("\u{2212}6,5"),
            Ok(-6.5),
            "a typographic minus, a comma"
        );
        assert_eq!(parse_db("-inf"), Ok(-60.0), "the fader's bottom");
        assert_eq!(parse_db("-INF dB"), Ok(-60.0));
        assert_eq!(parse_db("-∞"), Ok(-60.0));
        assert_eq!(parse_db("-infinity"), Ok(-60.0));
    }

    #[test]
    fn volumes_out_of_range_are_clamped_and_bad_text_refused() {
        assert_eq!(parse_db("+30"), Ok(12.0));
        assert_eq!(parse_db("-200 dB"), Ok(-60.0));
        for bad in [
            "", "  ", "loud", "dB", "-6 dBx", "inf", "+inf", "nan", "6-", "--6",
        ] {
            let err = parse_db(bad).unwrap_err();
            assert!(err.contains("isn't a volume"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn pans_read_as_the_readouts_show_them() {
        assert_eq!(parse_pan("C"), Ok(0.0));
        assert_eq!(parse_pan("c"), Ok(0.0));
        assert_eq!(parse_pan("centre"), Ok(0.0));
        assert_eq!(parse_pan("0"), Ok(0.0));
        assert_eq!(parse_pan("L30"), Ok(-0.3));
        assert_eq!(parse_pan("l 30"), Ok(-0.3));
        assert_eq!(parse_pan("R20"), Ok(0.2));
        assert_eq!(parse_pan("30L"), Ok(-0.3));
        assert_eq!(parse_pan("R20%"), Ok(0.2));
        assert_eq!(parse_pan("-30"), Ok(-0.3), "negative is left");
        assert_eq!(parse_pan("30"), Ok(0.3), "positive is right");
        assert_eq!(parse_pan("\u{2212}30"), Ok(-0.3));
        assert_eq!(parse_pan("L0"), Ok(0.0));
        // Every readout's own text reads back as its value.
        for pan in [-1.0, -0.3, 0.0, 0.25, 1.0] {
            let shown = crate::inspector::pan_label(pan);
            assert_eq!(parse_pan(&shown), Ok(pan), "{shown}");
        }
    }

    #[test]
    fn pans_out_of_range_are_clamped_and_bad_text_refused() {
        assert_eq!(parse_pan("L150"), Ok(-1.0));
        assert_eq!(parse_pan("R400"), Ok(1.0));
        assert_eq!(parse_pan("-250"), Ok(-1.0));
        for bad in ["", "L", "R", "left", "L-30", "LR", "X30", "nan", "inf"] {
            let err = parse_pan(bad).unwrap_err();
            assert!(err.contains("isn't a pan"), "{bad:?}: {err}");
        }
    }

    #[test]
    fn names_are_trimmed_and_never_empty() {
        assert_eq!(parse_name("  Lead guitar "), Ok("Lead guitar".into()));
        assert!(parse_name("   ").is_err());
        assert!(parse_name("").is_err());
    }

    /// The window and its pointer, frame by frame, without a screen.
    struct Window {
        ctx: egui::Context,
        events: Vec<Event>,
    }

    const FIELD: Pos2 = Pos2::new(60.0, 20.0);
    const ELSEWHERE: Pos2 = Pos2::new(300.0, 300.0);

    impl Window {
        fn new() -> Self {
            Self {
                ctx: egui::Context::default(),
                events: Vec::new(),
            }
        }

        fn frame(&mut self, mut ui: impl FnMut(&mut Ui)) {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                events: std::mem::take(&mut self.events),
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |root| ui(root));
            output.textures_delta.clear();
        }

        fn key(&mut self, key: Key) {
            for pressed in [true, false] {
                self.events.push(Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                });
            }
        }

        fn type_text(&mut self, text: &str) {
            self.events.push(Event::Text(text.into()));
        }

        fn click(&mut self, at: Pos2) {
            self.events.push(Event::PointerMoved(at));
            for pressed in [true, false] {
                self.events.push(Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
            }
        }
    }

    /// The field at the top left, opened on "Guitar L", run frame by frame: what it decided.
    fn run_name_field(script: impl Fn(&mut Window, usize)) -> (Vec<Decision<String>>, bool) {
        let mut window = Window::new();
        let id = Id::unique("name");
        open(&window.ctx, id, "Guitar L", 7);
        let mut decisions = Vec::new();
        for frame in 0..20 {
            script(&mut window, frame);
            window.frame(|ui| {
                if let Some(d) = field(ui, id, vec2(120.0, 20.0), FontId::default(), parse_name) {
                    decisions.push(d);
                }
            });
        }
        let still_open = is_open(&window.ctx, id);
        (decisions, still_open)
    }

    #[test]
    fn enter_commits_what_was_typed() {
        let (decisions, open) = run_name_field(|w, frame| match frame {
            2 => w.type_text("Lead guitar"),
            4 => w.key(Key::Enter),
            _ => {}
        });
        assert_eq!(decisions, [Decision::Commit("Lead guitar".to_string())]);
        assert!(!open);
    }

    #[test]
    fn typing_replaces_the_selected_text() {
        // Opened with its text selected: "Bass" is the whole name, not "Guitar LBass".
        let (decisions, _) = run_name_field(|w, frame| match frame {
            2 => w.type_text("Bass"),
            3 => w.key(Key::Enter),
            _ => {}
        });
        assert_eq!(decisions, [Decision::Commit("Bass".to_string())]);
    }

    #[test]
    fn clicking_away_commits_too() {
        let (decisions, open) = run_name_field(|w, frame| match frame {
            2 => w.type_text("Lead guitar"),
            4 => w.click(ELSEWHERE),
            _ => {}
        });
        assert_eq!(decisions, [Decision::Commit("Lead guitar".to_string())]);
        assert!(!open);
    }

    #[test]
    fn clicking_in_the_field_keeps_it_open() {
        let (decisions, open) = run_name_field(|w, frame| match frame {
            2 => w.type_text("Lead"),
            4 => w.click(FIELD),
            6 => w.type_text(" guitar"),
            _ => {}
        });
        assert!(decisions.is_empty(), "{decisions:?}");
        assert!(open);
    }

    #[test]
    fn escape_keeps_the_old_name() {
        let (decisions, open) = run_name_field(|w, frame| match frame {
            2 => w.type_text("Nope"),
            4 => w.key(Key::Escape),
            _ => {}
        });
        assert_eq!(decisions, [Decision::Cancel]);
        assert!(!open);
    }

    #[test]
    fn an_empty_name_is_refused_and_the_field_stays() {
        let (decisions, open) = run_name_field(|w, frame| match frame {
            2 => w.type_text("   "),
            4 => w.key(Key::Enter),
            _ => {}
        });
        assert!(
            matches!(&decisions[..], [Decision::Refuse(why)] if why.contains("needs a name")),
            "{decisions:?}"
        );
        assert!(open, "still open, to type a name or Escape");
        // Then Escape keeps the old name; or a name typed after the refusal commits.
        let (decisions, open) = run_name_field(|w, frame| match frame {
            2 => w.type_text("   "),
            4 => w.key(Key::Enter),
            6 => w.key(Key::Escape),
            _ => {}
        });
        assert!(matches!(decisions.last(), Some(Decision::Cancel)));
        assert!(!open);
        let (decisions, _) = run_name_field(|w, frame| match frame {
            2 => w.type_text("   "),
            4 => w.key(Key::Enter),
            6 => w.type_text("Rhythm"),
            8 => w.key(Key::Enter),
            _ => {}
        });
        assert_eq!(
            decisions.last(),
            Some(&Decision::Commit("Rhythm".to_string()))
        );
    }

    #[test]
    fn an_empty_name_clicked_away_from_keeps_the_old_one() {
        let (decisions, open) = run_name_field(|w, frame| match frame {
            2 => w.type_text(" "),
            4 => w.click(ELSEWHERE),
            _ => {}
        });
        assert_eq!(decisions, [Decision::Cancel]);
        assert!(!open);
    }

    #[test]
    fn a_field_left_undrawn_is_dropped_when_seen_again() {
        let mut window = Window::new();
        let id = Id::unique("name");
        open(&window.ctx, id, "Guitar L", 7);
        let mut decisions = Vec::new();
        for frame in 0..12 {
            if frame == 2 {
                window.type_text("Half");
            }
            // Not drawn in frames 4 to 7 (scrolled away): egui drops its focus.
            let drawn = !(4..8).contains(&frame);
            window.frame(|ui| {
                if drawn {
                    decisions.extend(field(
                        ui,
                        id,
                        vec2(120.0, 20.0),
                        FontId::default(),
                        parse_name,
                    ));
                }
            });
        }
        assert_eq!(decisions, [Decision::Cancel]);
    }

    #[test]
    fn an_orphaned_field_hands_back_its_target_and_text() {
        let ctx = egui::Context::default();
        let id = Id::unique("inspector");
        open(&ctx, id, "Guitar L", 7);
        assert_eq!(orphaned(&ctx, id, 7), None, "still on its track");
        assert_eq!(orphaned(&ctx, id, 8), Some((7, "Guitar L".to_string())));
        assert!(!is_open(&ctx, id));
    }

    #[test]
    fn a_readout_opens_on_a_click_and_types_a_volume() {
        let mut window = Window::new();
        let id = Id::unique("volume");
        let mut gain = 0.0f32;
        let mut typed = Vec::new();
        for frame in 0..16 {
            match frame {
                2 => window.click(Pos2::new(10.0, 10.0)),
                5 => window.type_text("-6.5 dB"),
                7 => window.key(Key::Enter),
                _ => {}
            }
            window.frame(|ui| {
                if let Some(v) = level(
                    ui,
                    id,
                    Level::Volume,
                    gain,
                    vec2(40.0, 20.0),
                    FontId::monospace(10.0),
                    true,
                    MASTER,
                ) {
                    typed.push(v);
                }
            });
            if let Some(&v) = typed.last() {
                gain = v;
            }
        }
        assert_eq!(typed, [-6.5]);
        assert!(!is_open(&window.ctx, id));
    }

    #[test]
    fn a_readout_that_isnt_editable_doesnt_open() {
        let mut window = Window::new();
        let id = Id::unique("pan");
        for frame in 0..6 {
            if frame == 2 {
                window.click(Pos2::new(10.0, 10.0));
            }
            window.frame(|ui| {
                level(
                    ui,
                    id,
                    Level::Pan,
                    0.0,
                    vec2(40.0, 20.0),
                    FontId::monospace(10.0),
                    false,
                    MASTER,
                );
            });
        }
        assert!(!is_open(&window.ctx, id));
    }

    /// 22.1's bug, kept as its proof: the header's old field asked for focus every frame right
    /// after drawing itself, which gives back the focus Enter (or a click elsewhere) has just
    /// taken, so `lost_focus` never fires and the typed name is never written. Only replacing
    /// the field (another rename) ever closed it, showing the old name.
    #[test]
    fn asking_for_focus_every_frame_never_lets_go() {
        let mut window = Window::new();
        let mut text = "Guitar L".to_string();
        let mut committed = None;
        for frame in 0..20 {
            match frame {
                2 => window.type_text(" lead"),
                4 => window.key(Key::Enter),
                8 => window.click(ELSEWHERE),
                _ => {}
            }
            window.frame(|ui| {
                let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(120.0));
                edit.request_focus();
                if edit.lost_focus() {
                    committed = Some(text.clone());
                }
            });
        }
        assert_eq!(text, "Guitar L lead", "the text was typed");
        assert_eq!(
            committed, None,
            "but neither Enter nor a click away let go of it"
        );
    }
}
