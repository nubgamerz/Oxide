//! The dockable window (dev plan 14.3): the Multitrack view, the mixer, the inspector, the
//! piano roll and each open audio file are panes in an `egui_tiles` tree, as tabs that drag
//! onto any side or the bottom of another pane to split it, or onto another tab bar to join
//! it. Layouts are saved by name in the settings, app-wide.
//!
//! The app's own flags stay the truth (`show_mixer`, `show_inspector`, `piano_roll`, the
//! open documents, `view_mode` and `editor.current`), set from many places and over MCP; each
//! frame [`reconcile`] makes the tree follow them. A pane switched off is hidden, not removed,
//! so it comes back where it was. Open files get one tab each beside the others; with none open
//! the Waveform tab shows where they will open (`Pane::Waveform(None)`), and a saved layout
//! keeps only that place, since files don't outlive a run.

use crate::app::DawApp;
use crate::theme;
use daw_core::api::ViewMode;
use daw_core::session::DocumentId;
use egui_tiles::{Container, LinearDir, SimplificationOptions, Tile, TileId, Tree, UiResponse};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pane {
    Multitrack,
    Mixer,
    Inspector,
    PianoRoll,
    /// The Waveform view's Diagnostics (Click Repair, Clip Repair): acts on the current file.
    Diagnostics,
    /// An open audio file's tab, or (`None`) the place files open, shown while none is.
    Waveform(Option<DocumentId>),
}

impl Pane {
    /// Its word in the manual's mark names (`pane.mixer`, `dock.tab.piano_roll`): every
    /// Waveform tab is `waveform`, the first one drawn kept.
    pub fn mark_key(self) -> &'static str {
        match self {
            Pane::Multitrack => "multitrack",
            Pane::Mixer => "mixer",
            Pane::Inspector => "inspector",
            Pane::PianoRoll => "piano_roll",
            Pane::Diagnostics => "diagnostics",
            Pane::Waveform(_) => "waveform",
        }
    }

    /// Its name in `get_layout`.
    pub fn name(self) -> &'static str {
        match self {
            Pane::Multitrack => "multitrack",
            Pane::Mixer => "mixer",
            Pane::Inspector => "inspector",
            Pane::PianoRoll => "piano_roll",
            Pane::Diagnostics => "diagnostics",
            Pane::Waveform(_) => "waveform",
        }
    }
}

/// What the app shows, for [`reconcile`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Wanted {
    pub mixer: bool,
    pub inspector: bool,
    pub piano_roll: bool,
    pub diagnostics: bool,
    /// The open audio files, in order.
    pub docs: Vec<DocumentId>,
}

pub type Layout = Tree<Pane>;

const TREE_ID: &str = "dock";

/// The built-in layout's name in View > Layouts and `set_layout`.
pub const DEFAULT: &str = "Default";

pub fn options() -> SimplificationOptions {
    SimplificationOptions {
        all_panes_must_have_tabs: true,
        ..Default::default()
    }
}

/// The window as Oxide has always laid it out: the Multitrack view (with the Waveform tab
/// beside it) above the piano roll and the mixer, the inspector on the right.
pub fn default_layout() -> Layout {
    let mut tiles = egui_tiles::Tiles::default();
    let multitrack = tiles.insert_pane(Pane::Multitrack);
    let waveform = tiles.insert_pane(Pane::Waveform(None));
    let main = tiles.insert_tab_tile(vec![multitrack, waveform]);
    let piano = tiles.insert_pane(Pane::PianoRoll);
    let piano = tiles.insert_tab_tile(vec![piano]);
    let mixer = tiles.insert_pane(Pane::Mixer);
    let mixer = tiles.insert_tab_tile(vec![mixer]);
    let column = tiles.insert_vertical_tile(vec![main, piano, mixer]);
    let inspector = tiles.insert_pane(Pane::Inspector);
    let inspector = tiles.insert_tab_tile(vec![inspector]);
    let root = tiles.insert_horizontal_tile(vec![column, inspector]);
    // The mixer as tall as its panel was (about half the window): its strips need it.
    set_shares(
        &mut tiles,
        column,
        &[(main, 1.9), (piano, 1.3), (mixer, 2.1)],
    );
    set_shares(&mut tiles, root, &[(column, 3.8), (inspector, 1.0)]);
    let mut tree = Tree::new(egui::Id::unique(TREE_ID), root, tiles);
    if let Some(t) = tree.tiles.find_pane(&Pane::Multitrack) {
        tree.make_active(|id, _| id == t);
    }
    // Shown when asked for (the M key, a MIDI clip opened).
    for pane in [Pane::PianoRoll, Pane::Mixer] {
        if let Some(t) = find(&tree, pane) {
            tree.set_visible(t, false);
        }
    }
    tree
}

fn set_shares(tiles: &mut egui_tiles::Tiles<Pane>, linear: TileId, shares: &[(TileId, f32)]) {
    if let Some(Tile::Container(Container::Linear(l))) = tiles.get_mut(linear) {
        for &(id, share) in shares {
            l.shares.set_share(id, share);
        }
    }
}

/// The tile showing `pane`.
pub fn find(tree: &Layout, pane: Pane) -> Option<TileId> {
    tree.tiles.find_pane(&pane)
}

/// Where `pane`'s tab was drawn this frame, if it has one on screen (the screenshot check
/// points at it).
pub fn tab_rect(ctx: &egui::Context, dock: &Dock, pane: Pane) -> Option<egui::Rect> {
    let tile = find(&dock.tree, pane)?;
    ctx.read_response(tile.egui_id(dock.tree.id()))
        .map(|r| r.rect)
}

/// Every pane in the tree, in order (depth first).
pub fn panes(tree: &Layout) -> Vec<(TileId, Pane)> {
    fn walk(tree: &Layout, id: TileId, out: &mut Vec<(TileId, Pane)>) {
        match tree.tiles.get(id) {
            Some(Tile::Pane(p)) => out.push((id, *p)),
            Some(Tile::Container(c)) => {
                for &child in c.children() {
                    walk(tree, child, out);
                }
            }
            None => {}
        }
    }
    let mut out = Vec::new();
    if let Some(root) = tree.root() {
        walk(tree, root, &mut out);
    }
    out
}

/// The tabs container holding `tile`.
fn tabs_of(tree: &Layout, tile: TileId) -> Option<TileId> {
    let parent = tree.tiles.parent_of(tile)?;
    matches!(
        tree.tiles.get(parent),
        Some(Tile::Container(Container::Tabs(_)))
    )
    .then_some(parent)
}

/// Put `new` beside `anchor` in direction `dir` (after it, or before), as a share `share` of
/// the anchor's: into the anchor's parent when that runs the same way, else in a new one.
fn insert_beside(
    tree: &mut Layout,
    anchor: TileId,
    new: TileId,
    dir: LinearDir,
    after: bool,
    share: f32,
) {
    let parent = tree.tiles.parent_of(anchor);
    if let Some(parent) = parent {
        if let Some(Tile::Container(Container::Linear(l))) = tree.tiles.get_mut(parent) {
            if l.dir == dir {
                let at = l.children.iter().position(|&c| c == anchor).unwrap_or(0);
                l.children.insert(if after { at + 1 } else { at }, new);
                let base = l.shares[anchor];
                l.shares.set_share(new, base * share);
                return;
            }
        }
    }
    let children = if after {
        vec![anchor, new]
    } else {
        vec![new, anchor]
    };
    let linear = tree
        .tiles
        .insert_container(egui_tiles::Linear::new(dir, children));
    set_shares(&mut tree.tiles, linear, &[(anchor, 1.0), (new, share)]);
    match parent {
        Some(parent) => {
            if let Some(Tile::Container(c)) = tree.tiles.get_mut(parent) {
                let _ = c.replace_child(anchor, linear);
            }
        }
        None => tree.root = Some(linear),
    }
}

/// Make `tree` show what `wanted` asks: every pane in it (a layout without one gets it at its
/// usual place), the mixer, inspector and piano roll shown or hidden, one tab per open file
/// (new ones beside the others) and the Waveform tab only while no file is open. A tree
/// without the Multitrack view is replaced by the default.
pub fn reconcile(tree: &mut Layout, wanted: &Wanted) {
    let Some(multitrack) = find(tree, Pane::Multitrack) else {
        *tree = default_layout();
        return reconcile(tree, wanted);
    };
    let main = tabs_of(tree, multitrack).unwrap_or(multitrack);
    // One place for files.
    let slots: Vec<TileId> = panes(tree)
        .into_iter()
        .filter(|p| p.1 == Pane::Waveform(None))
        .map(|p| p.0)
        .collect();
    let slot = match slots.split_first() {
        Some((&first, rest)) => {
            for &extra in rest {
                tree.remove_recursively(extra);
            }
            first
        }
        None => {
            let slot = tree.tiles.insert_pane(Pane::Waveform(None));
            add_tab(tree, main, slot);
            slot
        }
    };
    // The mixer, piano roll, inspector and Diagnostics: at their usual place if missing.
    for (pane, show) in [
        (Pane::PianoRoll, wanted.piano_roll),
        (Pane::Mixer, wanted.mixer),
        (Pane::Inspector, wanted.inspector),
        (Pane::Diagnostics, wanted.diagnostics),
    ] {
        let tile = match find(tree, pane) {
            Some(t) => t,
            None => {
                let tile = tree.tiles.insert_pane(pane);
                let main = tabs_of(tree, multitrack).unwrap_or(multitrack);
                // The Diagnostics: a tab beside the inspector, a list in a side panel.
                let inspector = find(tree, Pane::Inspector).and_then(|i| tabs_of(tree, i));
                match (pane, inspector) {
                    (Pane::Diagnostics, Some(tabs)) => add_tab(tree, tabs, tile),
                    // Under the main tabs, so the piano roll sits above a mixer already there.
                    (Pane::PianoRoll, _) => {
                        let new = tree.tiles.insert_tab_tile(vec![tile]);
                        insert_beside(tree, main, new, LinearDir::Vertical, true, 0.5)
                    }
                    (Pane::Mixer, _) => {
                        let new = tree.tiles.insert_tab_tile(vec![tile]);
                        let under = find(tree, Pane::PianoRoll)
                            .and_then(|p| tabs_of(tree, p))
                            .filter(|t| tree.tiles.parent_of(*t) == tree.tiles.parent_of(main))
                            .unwrap_or(main);
                        insert_beside(tree, under, new, LinearDir::Vertical, true, 0.6)
                    }
                    _ => {
                        let new = tree.tiles.insert_tab_tile(vec![tile]);
                        let root = tree.root().unwrap_or(main);
                        insert_beside(tree, root, new, LinearDir::Horizontal, true, 0.26)
                    }
                }
                tile
            }
        };
        tree.set_visible(tile, show);
    }
    // Files: closed ones go; new ones join the last file's tabs, or the Waveform tab's.
    for (tile, pane) in panes(tree) {
        if let Pane::Waveform(Some(doc)) = pane {
            if !wanted.docs.contains(&doc) {
                tree.remove_recursively(tile);
            }
        }
    }
    for &doc in &wanted.docs {
        if find(tree, Pane::Waveform(Some(doc))).is_some() {
            continue;
        }
        let beside = panes(tree)
            .into_iter()
            .filter(|p| matches!(p.1, Pane::Waveform(Some(_))))
            .map(|p| p.0)
            .next_back()
            .unwrap_or(slot);
        let tabs = tabs_of(tree, beside).unwrap_or(main);
        let new = tree.tiles.insert_pane(Pane::Waveform(Some(doc)));
        add_tab(tree, tabs, new);
    }
    tree.set_visible(slot, wanted.docs.is_empty());
    tree.simplify(&options());
}

fn add_tab(tree: &mut Layout, tabs: TileId, new: TileId) {
    match tree.tiles.get_mut(tabs) {
        Some(Tile::Container(c)) => c.add_child(new),
        _ => {
            let wrapped = tree.tiles.insert_tab_tile(vec![new]);
            insert_beside(tree, tabs, wrapped, LinearDir::Horizontal, true, 1.0);
        }
    }
}

/// `tree` as a layout to keep: the files' tabs become the one place files open (where the
/// first of them was), shown, since the files themselves don't outlive a run.
pub fn for_saving(tree: &Layout) -> Layout {
    let mut tree = tree.clone();
    let docs: Vec<TileId> = panes(&tree)
        .into_iter()
        .filter(|p| matches!(p.1, Pane::Waveform(Some(_))))
        .map(|p| p.0)
        .collect();
    if let Some((&first, rest)) = docs.split_first() {
        if let Some(slot) = find(&tree, Pane::Waveform(None)) {
            tree.remove_recursively(slot);
        }
        for &tile in rest {
            tree.remove_recursively(tile);
        }
        if let Some(Tile::Pane(p)) = tree.tiles.get_mut(first) {
            *p = Pane::Waveform(None);
        }
    }
    if let Some(slot) = find(&tree, Pane::Waveform(None)) {
        tree.set_visible(slot, true);
    }
    tree.simplify(&options());
    tree
}

/// The layout as `get_layout` describes it: containers with their children and panes with
/// whether they show.
pub fn describe(tree: &Layout) -> serde_json::Value {
    use serde_json::json;
    fn walk(tree: &Layout, id: TileId) -> serde_json::Value {
        let visible = tree.tiles.is_visible_in_layout(id);
        match tree.tiles.get(id) {
            Some(Tile::Pane(p)) => {
                let mut v = json!({"pane": p.name(), "visible": visible});
                if let Pane::Waveform(Some(doc)) = p {
                    v["document"] = json!(doc);
                }
                v
            }
            Some(Tile::Container(c)) => {
                let children: Vec<_> = c.children().map(|&k| walk(tree, k)).collect();
                match c {
                    Container::Tabs(t) => {
                        let active = t
                            .active
                            .and_then(|a| tree.tiles.get_pane(&a))
                            .map(|p| p.name());
                        json!({"type": "tabs", "active": active, "visible": visible, "tabs": children})
                    }
                    Container::Linear(l) => {
                        let shares: Vec<f32> = c.children().map(|&k| l.shares[k]).collect();
                        let kind = match l.dir {
                            LinearDir::Horizontal => "horizontal",
                            LinearDir::Vertical => "vertical",
                        };
                        json!({"type": kind, "shares": shares, "visible": visible, "children": children})
                    }
                    Container::Grid(_) => {
                        json!({"type": "grid", "visible": visible, "children": children})
                    }
                }
            }
            None => serde_json::Value::Null,
        }
    }
    tree.root()
        .map_or(serde_json::Value::Null, |root| walk(tree, root))
}

/// The dock's state in the app.
pub struct Dock {
    pub tree: Layout,
    /// The pane last brought to the front for the app's view (`view_mode` and the current
    /// file), so a change from elsewhere (a menu, MCP) brings its tab forward once.
    synced: Option<Pane>,
    /// The flags last reconciled, so a pane switched on comes to the front of its tabs.
    shown: Wanted,
}

impl Dock {
    pub fn new(tree: Layout) -> Self {
        Self {
            tree,
            synced: None,
            shown: Wanted::default(),
        }
    }
}

/// Empty, and cheap: [`reconcile`] turns an empty tree into the default layout.
impl Default for Dock {
    fn default() -> Self {
        Self::new(Tree::empty(egui::Id::unique(TREE_ID)))
    }
}

/// What the app asks for now.
pub fn wanted(app: &DawApp) -> Wanted {
    Wanted {
        mixer: app.show_mixer,
        inspector: app.show_inspector,
        piano_roll: app.piano_roll.is_some(),
        diagnostics: app.show_diagnostics,
        docs: app.session.documents().iter().map(|d| d.id).collect(),
    }
}

/// The pane the app's view is on: the Multitrack view, or the current file's tab.
fn focus(app: &DawApp) -> Pane {
    match app.view_mode {
        ViewMode::Multitrack => Pane::Multitrack,
        ViewMode::Waveform => Pane::Waveform(app.editor.current),
    }
}

/// The flags a layout shows: loading one shows the mixer and inspector as it has them.
pub fn apply_flags(app: &mut DawApp, tree: &Layout) {
    let shown = |pane| find(tree, pane).is_some_and(|t| tree.tiles.is_visible(t));
    app.show_mixer = shown(Pane::Mixer);
    app.show_inspector = shown(Pane::Inspector);
    app.show_diagnostics = shown(Pane::Diagnostics);
}

/// Draw the panes in `ui` (the window below the transport).
pub fn show(app: &mut DawApp, ui: &mut egui::Ui) {
    let want = wanted(app);
    if let Some(p) = &app.piano_roll {
        app.last_piano_clip = Some(p.clip);
    }
    let mut dock = std::mem::take(&mut app.dock);
    reconcile(&mut dock.tree, &want);
    // A pane just switched on comes to the front of its tabs.
    for (pane, now, before) in [
        (Pane::Mixer, want.mixer, dock.shown.mixer),
        (Pane::Inspector, want.inspector, dock.shown.inspector),
        (Pane::PianoRoll, want.piano_roll, dock.shown.piano_roll),
        (Pane::Diagnostics, want.diagnostics, dock.shown.diagnostics),
    ] {
        if now && !before {
            if let Some(t) = find(&dock.tree, pane) {
                dock.tree.make_active(|id, _| id == t);
            }
        }
    }
    dock.shown = want;
    // The view moved elsewhere (a menu, a double-click, MCP): its tab comes forward.
    let focus = focus(app);
    if dock.synced != Some(focus) {
        if let Some(t) = find(&dock.tree, focus) {
            dock.tree.make_active(|id, _| id == t);
        }
        dock.synced = Some(focus);
    }
    let mut behavior = Behavior {
        app,
        closed: Vec::new(),
        pressed: None,
        ctx: ui.ctx().clone(),
        tree: dock.tree.id(),
    };
    dock.tree.ui(&mut behavior, ui);
    // egui_tiles shows `Grab` over a tab bar's empty background (and Windows draws it as the
    // four-way move cross): an arrow there too, until something is actually dragged.
    let dragging = ui.ctx().dragged_id().is_some();
    ui.ctx().output_mut(|o| {
        if o.cursor_icon == egui::CursorIcon::Grab && !dragging {
            o.cursor_icon = egui::CursorIcon::Default;
        }
    });
    let Behavior {
        app,
        closed,
        pressed,
        ..
    } = behavior;
    for pane in closed {
        match pane {
            Pane::Mixer => app.show_mixer = false,
            Pane::Inspector => app.show_inspector = false,
            Pane::PianoRoll => app.piano_roll = None,
            Pane::Diagnostics => app.show_diagnostics = false,
            Pane::Waveform(Some(doc)) => crate::waveform::close_file(app, doc),
            Pane::Multitrack | Pane::Waveform(None) => {}
        }
    }
    // A click in a pane (or on its tab) makes it the view the keys and menus act on.
    if let Some(pane) = pressed {
        match pane {
            Pane::Multitrack => app.view_mode = ViewMode::Multitrack,
            Pane::Waveform(doc) => {
                app.view_mode = ViewMode::Waveform;
                if doc.is_some() {
                    app.editor.current = doc;
                }
            }
            _ => {}
        }
        dock.synced = Some(self::focus(app));
    }
    app.dock = dock;
}

struct Behavior<'a> {
    app: &'a mut DawApp,
    closed: Vec<Pane>,
    pressed: Option<Pane>,
    /// To ask whether a tab is hovered: egui_tiles' `TabState` doesn't say, but a tab's widget
    /// id is its tile's (`TileId::egui_id` of the tree's id), and its response this frame
    /// knows.
    ctx: egui::Context,
    tree: egui::Id,
}

impl Behavior<'_> {
    /// The pointer is over tile `tile`'s tab (and nothing is being dragged).
    fn tab_hovered(&self, tile: TileId) -> bool {
        self.ctx.dragged_id().is_none()
            && self
                .ctx
                .read_response(tile.egui_id(self.tree))
                .is_some_and(|r| r.hovered())
    }
}

impl egui_tiles::Behavior<Pane> for Behavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, tile_id: TileId, pane: &mut Pane) -> UiResponse {
        if ui.input(|i| i.pointer.any_pressed()) && ui.ui_contains_pointer() {
            self.pressed = Some(*pane);
        }
        let app = &mut *self.app;
        let key = pane.mark_key();
        crate::marks::put_with(ui, || format!("pane.{key}"), ui.max_rect());
        // F1 over this pane: its page in the manual (27.3).
        let place = match *pane {
            Pane::Multitrack => crate::help::Place::Multitrack,
            Pane::Mixer => crate::help::Place::Mixer,
            Pane::Inspector => crate::help::Place::Inspector,
            Pane::PianoRoll => crate::help::Place::PianoRoll,
            Pane::Diagnostics => crate::help::Place::Diagnostics,
            Pane::Waveform(_) => crate::help::Place::Waveform,
        };
        app.help.note(ui.ctx(), place, ui.layer_id(), ui.max_rect());
        match *pane {
            Pane::Multitrack => {
                egui::Frame::new()
                    .fill(theme::LANE)
                    .show(ui, |ui| crate::arrange::show(app, ui));
            }
            Pane::Mixer => {
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(4)
                    .show(ui, |ui| crate::mixer::show(app, ui));
            }
            Pane::Inspector => {
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(6)
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        crate::inspector::show(app, ui)
                    });
            }
            Pane::PianoRoll => {
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(4)
                    .show(ui, |ui| crate::piano_roll::show(app, ui));
            }
            Pane::Diagnostics => {
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(6)
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        crate::diagnostics::show(app, ui)
                    });
            }
            Pane::Waveform(doc) => {
                // Shown beside another file's tab (a split): this one is drawn as the
                // current file for the moment, since the editor keeps one file's state.
                let current = app.editor.current;
                if doc.is_some() {
                    app.editor.current = doc;
                }
                egui::Panel::left(egui::Id::unique(("waveform_rack", tile_id)))
                    .resizable(true)
                    .default_size(230.0)
                    .min_size(180.0)
                    .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(6))
                    .show(ui, |ui| crate::waveform::rack_panel(app, ui));
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(theme::LANE).inner_margin(4))
                    .show(ui, |ui| crate::waveform::show(app, ui));
                // A file closed from its toolbar picks the next current one itself.
                if doc.is_some() && app.editor.current == doc {
                    app.editor.current = current.filter(|c| app.session.document(*c).is_some());
                    if app.editor.current.is_none() {
                        app.editor.current = doc;
                    }
                }
            }
        }
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        match pane {
            Pane::Multitrack => "Multitrack".into(),
            Pane::Mixer => "Mixer".into(),
            Pane::Inspector => "Inspector".into(),
            Pane::PianoRoll => "Piano roll".into(),
            Pane::Diagnostics => "Diagnostics".into(),
            Pane::Waveform(None) => "Waveform".into(),
            Pane::Waveform(Some(doc)) => match self.app.session.document(*doc) {
                Some(d) => {
                    let name = d
                        .path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    if d.is_dirty() {
                        format!("{name} *").into()
                    } else {
                        name.into()
                    }
                }
                None => "Waveform".into(),
            },
        }
    }

    fn is_tab_closable(&self, tiles: &egui_tiles::Tiles<Pane>, tile_id: TileId) -> bool {
        !matches!(
            tiles.get_pane(&tile_id),
            Some(Pane::Multitrack | Pane::Waveform(None)) | None
        )
    }

    fn on_tab_close(&mut self, tiles: &mut egui_tiles::Tiles<Pane>, tile_id: TileId) -> bool {
        // The app's flags decide; `reconcile` hides or removes the tile next frame (a file
        // with unsaved edits asks first).
        if let Some(pane) = tiles.get_pane(&tile_id) {
            self.closed.push(*pane);
        }
        false
    }

    fn on_tab_button(
        &mut self,
        tiles: &mut egui_tiles::Tiles<Pane>,
        tile_id: TileId,
        button_response: egui::Response,
    ) -> egui::Response {
        if let Some(pane) = tiles.get_pane(&tile_id) {
            let key = pane.mark_key();
            crate::marks::put_rect_with(|| format!("dock.tab.{key}"), button_response.rect);
        }
        if button_response.clicked() {
            if let Some(pane) = tiles.get_pane(&tile_id) {
                self.pressed = Some(*pane);
            }
        }
        button_response
    }

    fn simplification_options(&self) -> SimplificationOptions {
        options()
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        2.0
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        22.0
    }

    fn tab_bar_color(&self, _visuals: &egui::Visuals) -> egui::Color32 {
        theme::BG
    }

    /// A tab is clicked, mostly: the arrow, not egui_tiles' `Grab` (which Windows draws as the
    /// four-way move cross). Dragging one to re-dock still works, and shows while it does.
    fn tab_hover_cursor_icon(&self) -> egui::CursorIcon {
        egui::CursorIcon::Default
    }

    /// The active tab joins its pane; the one under the pointer lights up, so it's clear which
    /// a click picks.
    fn tab_bg_color(
        &self,
        visuals: &egui::Visuals,
        _tiles: &egui_tiles::Tiles<Pane>,
        tile_id: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Color32 {
        if state.active {
            visuals.panel_fill
        } else if self.tab_hovered(tile_id) {
            // A step up from the bar, under the active tab (which joins its pane), with a dim
            // accent outline (`tab_outline_stroke`).
            theme::LANE
        } else {
            egui::Color32::TRANSPARENT
        }
    }

    fn tab_outline_stroke(
        &self,
        visuals: &egui::Visuals,
        _tiles: &egui_tiles::Tiles<Pane>,
        tile_id: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Stroke {
        if state.active {
            egui::Stroke::new(1.0, visuals.widgets.active.bg_fill)
        } else if self.tab_hovered(tile_id) {
            egui::Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.55))
        } else {
            egui::Stroke::NONE
        }
    }

    fn tab_text_color(
        &self,
        visuals: &egui::Visuals,
        _tiles: &egui_tiles::Tiles<Pane>,
        tile_id: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Color32 {
        if state.active {
            visuals.widgets.active.text_color()
        } else if self.tab_hovered(tile_id) {
            visuals.widgets.hovered.text_color()
        } else {
            visuals.widgets.noninteractive.text_color()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible(tree: &Layout, pane: Pane) -> bool {
        find(tree, pane).is_some_and(|t| tree.tiles.is_visible_in_layout(t))
    }

    #[test]
    fn the_tree_follows_the_flags_and_keeps_places() {
        let mut tree = default_layout();
        let mut want = Wanted {
            inspector: true,
            ..Default::default()
        };
        reconcile(&mut tree, &want);
        assert!(visible(&tree, Pane::Multitrack) && visible(&tree, Pane::Inspector));
        assert!(!visible(&tree, Pane::Mixer) && !visible(&tree, Pane::PianoRoll));
        // The Diagnostics: there (beside the inspector) but hidden until asked for, and back
        // in the same tabs when hidden and shown again.
        let tabs = |tree: &Layout, pane| tabs_of(tree, find(tree, pane).unwrap());
        assert!(!visible(&tree, Pane::Diagnostics));
        assert_eq!(tabs(&tree, Pane::Diagnostics), tabs(&tree, Pane::Inspector));
        for show in [true, false, true] {
            want.diagnostics = show;
            reconcile(&mut tree, &want);
            assert_eq!(visible(&tree, Pane::Diagnostics), show);
            assert_eq!(tabs(&tree, Pane::Diagnostics), tabs(&tree, Pane::Inspector));
        }
        assert_eq!(
            describe(&tree).to_string().matches("diagnostics").count(),
            1
        );
        want.diagnostics = false;
        reconcile(&mut tree, &want);
        assert!(
            visible(&tree, Pane::Waveform(None)),
            "where files will open"
        );

        // Two files open beside the Waveform tab, which hides; one closes; then the other.
        want.docs = vec![3, 5];
        reconcile(&mut tree, &want);
        let slot_tabs = tabs_of(&tree, find(&tree, Pane::Waveform(None)).unwrap());
        for doc in [3, 5] {
            let tile = find(&tree, Pane::Waveform(Some(doc))).unwrap();
            assert_eq!(tabs_of(&tree, tile), slot_tabs);
        }
        assert!(!tree
            .tiles
            .is_visible(find(&tree, Pane::Waveform(None)).unwrap()));
        want.docs = vec![5];
        reconcile(&mut tree, &want);
        assert!(find(&tree, Pane::Waveform(Some(3))).is_none());
        want.docs.clear();
        reconcile(&mut tree, &want);
        assert!(visible(&tree, Pane::Waveform(None)));

        // The mixer moved to the right of the inspector, hidden and shown again: still there.
        want.mixer = true;
        reconcile(&mut tree, &want);
        let mixer = find(&tree, Pane::Mixer).unwrap();
        let inspector_tabs = tabs_of(&tree, find(&tree, Pane::Inspector).unwrap()).unwrap();
        let mixer_tabs = tabs_of(&tree, mixer).unwrap();
        if let Some(Tile::Container(c)) = tree.tiles.get_mut(mixer_tabs) {
            c.remove_child(mixer);
        }
        if let Some(Tile::Container(c)) = tree.tiles.get_mut(inspector_tabs) {
            c.add_child(mixer);
        }
        tree.simplify(&options());
        for show in [false, true] {
            want.mixer = show;
            reconcile(&mut tree, &want);
            assert_eq!(
                tabs_of(&tree, find(&tree, Pane::Mixer).unwrap()),
                Some(inspector_tabs)
            );
            assert_eq!(visible(&tree, Pane::Mixer), show);
        }
    }

    #[test]
    fn missing_panes_come_back_at_their_place() {
        // A layout of the Multitrack view alone.
        let mut tree = Tree::new_tabs(egui::Id::unique(TREE_ID), vec![Pane::Multitrack]);
        let want = Wanted {
            mixer: true,
            inspector: true,
            piano_roll: true,
            diagnostics: true,
            docs: vec![1],
        };
        reconcile(&mut tree, &want);
        for pane in [
            Pane::Multitrack,
            Pane::Mixer,
            Pane::Inspector,
            Pane::PianoRoll,
            Pane::Diagnostics,
            Pane::Waveform(Some(1)),
        ] {
            assert!(
                visible(&tree, pane) || pane == Pane::Waveform(Some(1)),
                "{pane:?}"
            );
        }
        // The file shares the Multitrack view's tabs; the piano roll sits above the mixer.
        let main = tabs_of(&tree, find(&tree, Pane::Multitrack).unwrap());
        assert_eq!(
            tabs_of(&tree, find(&tree, Pane::Waveform(Some(1))).unwrap()),
            main
        );
        let rect_order = panes(&tree)
            .into_iter()
            .map(|p| p.1)
            .filter(|p| matches!(p, Pane::PianoRoll | Pane::Mixer))
            .collect::<Vec<_>>();
        assert_eq!(rect_order, [Pane::PianoRoll, Pane::Mixer]);
        // The Diagnostics a tab beside the inspector.
        let inspector = tabs_of(&tree, find(&tree, Pane::Inspector).unwrap());
        assert_eq!(
            tabs_of(&tree, find(&tree, Pane::Diagnostics).unwrap()),
            inspector
        );
        // No Multitrack view at all: the default.
        let mut odd = Tree::new_tabs(egui::Id::unique(TREE_ID), vec![Pane::Mixer]);
        reconcile(&mut odd, &want);
        assert!(find(&odd, Pane::Multitrack).is_some());
    }

    #[test]
    fn a_saved_layout_keeps_where_files_open() {
        let mut tree = default_layout();
        let mut want = Wanted {
            mixer: true,
            docs: vec![7, 8],
            ..Default::default()
        };
        reconcile(&mut tree, &want);
        // The files' tabs dragged to a new row at the bottom.
        let a = find(&tree, Pane::Waveform(Some(7))).unwrap();
        let b = find(&tree, Pane::Waveform(Some(8))).unwrap();
        let from = tabs_of(&tree, a).unwrap();
        if let Some(Tile::Container(c)) = tree.tiles.get_mut(from) {
            c.retain(|t| t != a && t != b);
        }
        let bottom = tree.tiles.insert_tab_tile(vec![a, b]);
        let root = tree.root().unwrap();
        insert_beside(&mut tree, root, bottom, LinearDir::Vertical, true, 0.3);
        tree.simplify(&options());

        let saved = for_saving(&tree);
        let json = serde_json::to_string(&saved).unwrap();
        let mut back: Layout = serde_json::from_str(&json).unwrap();
        assert!(panes(&back)
            .iter()
            .all(|p| !matches!(p.1, Pane::Waveform(Some(_)))));
        let slot = find(&back, Pane::Waveform(None)).unwrap();
        assert!(back.tiles.is_visible(slot));
        // Opening two files again puts them there, at the bottom, beside nothing else.
        want.docs = vec![1, 2];
        reconcile(&mut back, &want);
        let tabs = tabs_of(&back, find(&back, Pane::Waveform(Some(1))).unwrap()).unwrap();
        assert_eq!(
            tabs_of(&back, find(&back, Pane::Waveform(Some(2))).unwrap()),
            Some(tabs)
        );
        assert_ne!(
            Some(tabs),
            tabs_of(&back, find(&back, Pane::Multitrack).unwrap()),
            "not with the Multitrack view"
        );
        assert_eq!(back.root(), Some(back.tiles.parent_of(tabs).unwrap()));
        assert_eq!(describe(&back)["type"], "vertical");
    }
}
