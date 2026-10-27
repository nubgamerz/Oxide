//! The reference checker: what a page may point at, and what is wrong when it can't.
//!
//! Every reference a page makes (another page, a heading on it, a screenshot, a highlight on a
//! screenshot, an MCP action) is resolved against [`Known`] while the page renders. Problems
//! are collected, never thrown, so one run reports every break. New kinds of reference (the
//! built-in effects, the settings rows, 27.5) add a set to `Known` and a [`RefKind`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::figure::Marks;

/// How bad a problem is: errors fail the build (and the quick tier), warnings are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

/// What kind of reference a problem is about, so tests (and later tools) can tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// A page that doesn't exist (`[[page:x]]`, `[text](x.md)`).
    Page,
    /// A heading anchor that doesn't exist on its page (`[[page:x#y]]`).
    Anchor,
    /// Two headings on one page with the same anchor.
    DuplicateAnchor,
    /// A figure's screenshot (PNG or its marks JSON) that isn't there.
    Shot,
    /// A highlight or crop naming a mark the shot's JSON doesn't have.
    Mark,
    /// An MCP action that isn't in the registry.
    Action,
    /// A built-in effect that doesn't exist, isn't documented, or is documented twice
    /// (`::: params`, `[[effect:...]]`).
    Effect,
    /// An MCP example that can't be read, names an action that isn't there or sits in
    /// another area's file; an action without one.
    Example,
    /// The table of contents and the page files disagree.
    Contents,
    /// Anything else: bad syntax, a page without a title, an unknown `[[kind:...]]`.
    Syntax,
}

/// One problem found while building, with where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub severity: Severity,
    pub kind: RefKind,
    /// The source file, relative to the manual's folder (`troubleshooting/choppy-audio.md`).
    pub file: String,
    /// 1-based; 0 when the problem is about the whole file.
    pub line: usize,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}: {}", self.file, self.line, self.message)
        } else {
            write!(f, "{}: {}", self.file, self.message)
        }
    }
}

/// The problems found so far, for one build.
#[derive(Debug, Default)]
pub struct Problems {
    pub list: Vec<Problem>,
}

impl Problems {
    pub fn error(&mut self, kind: RefKind, file: &str, line: usize, message: impl Into<String>) {
        self.push(Severity::Error, kind, file, line, message);
    }

    pub fn warn(&mut self, kind: RefKind, file: &str, line: usize, message: impl Into<String>) {
        self.push(Severity::Warning, kind, file, line, message);
    }

    pub fn push(
        &mut self,
        severity: Severity,
        kind: RefKind,
        file: &str,
        line: usize,
        message: impl Into<String>,
    ) {
        self.list.push(Problem {
            severity,
            kind,
            file: file.to_string(),
            line,
            message: message.into(),
        });
    }

    pub fn errors(&self) -> usize {
        self.list
            .iter()
            .filter(|p| p.severity == Severity::Error)
            .count()
    }
}

/// A page as the checker knows it: its title and its headings' anchors (anchor to heading
/// text, which is also a link's default text).
#[derive(Debug, Clone, Default)]
pub struct PageInfo {
    pub title: String,
    pub anchors: BTreeMap<String, String>,
    /// The built-in effects whose `::: params` table is on the page: id, the anchor of the
    /// heading it sits under (empty: the page's top) and its line.
    pub effects: Vec<(String, String, usize)>,
}

/// A screenshot as the checker knows it.
#[derive(Debug, Clone, Default)]
pub struct ShotInfo {
    /// The PNG is there.
    pub image: bool,
    /// Its marks (`shots/<name>.json`), if there and readable.
    pub marks: Option<Marks>,
}

/// Everything a page may refer to.
#[derive(Debug, Default)]
pub struct Known {
    /// Page id (the file's stem) to its title and anchors.
    pub pages: BTreeMap<String, PageInfo>,
    /// The MCP actions' names, each with its reference page's id.
    pub actions: BTreeMap<String, String>,
    /// Screenshots by name.
    pub shots: BTreeMap<String, ShotInfo>,
    /// The shots `shots.toml` lists (the manual tier takes these), when the manual has one:
    /// a figure's shot must be among them.
    pub listed_shots: Option<BTreeSet<String>>,
    /// A missing screenshot is an error, not a warning (27.2's shots are generated, so a
    /// missing one is then a broken reference).
    pub strict_shots: bool,
    /// The built-in effects (27.5, `effects.json`).
    pub effects: BTreeMap<String, crate::effects::Effect>,
    /// The rate their delays are given at.
    pub effect_rate: f64,
    /// Where each effect is documented: its page id and heading anchor.
    pub effect_places: BTreeMap<String, (String, String)>,
}

impl Known {
    /// Checks `[[page:id#anchor]]` (or a Markdown link to `id.md#anchor`). `page` is the
    /// page being rendered, used when `id` is empty (`[[page:#anchor]]`). Returns the href
    /// and the link's default text when it resolves.
    pub fn resolve_page(
        &self,
        here: &str,
        target: &str,
        problems: &mut Problems,
        file: &str,
        line: usize,
    ) -> Option<(String, String)> {
        let (id, anchor) = match target.split_once('#') {
            Some((id, anchor)) => (id, Some(anchor)),
            None => (target, None),
        };
        let id = if id.is_empty() { here } else { id };
        let Some(page) = self.pages.get(id) else {
            problems.error(
                RefKind::Page,
                file,
                line,
                format!("link to a page that doesn't exist: \"{id}\""),
            );
            return None;
        };
        let file_name = if id == here {
            String::new()
        } else {
            format!("{id}.html")
        };
        match anchor {
            None => Some((
                if file_name.is_empty() {
                    "#".to_string()
                } else {
                    file_name
                },
                page.title.clone(),
            )),
            Some(anchor) => match page.anchors.get(anchor) {
                Some(heading) => Some((format!("{file_name}#{anchor}"), heading.clone())),
                None => {
                    problems.error(
                        RefKind::Anchor,
                        file,
                        line,
                        format!("link to a heading that doesn't exist: \"{id}#{anchor}\""),
                    );
                    None
                }
            },
        }
    }

    /// Checks `[[effect:id]]`: the page and heading where the effect's table is, and the
    /// effect's name.
    pub fn resolve_effect(
        &self,
        here: &str,
        id: &str,
        problems: &mut Problems,
        file: &str,
        line: usize,
    ) -> Option<(String, String)> {
        let Some(effect) = self.effects.get(id) else {
            problems.error(
                RefKind::Effect,
                file,
                line,
                format!("no built-in effect has the id \"{id}\""),
            );
            return None;
        };
        let Some((page, anchor)) = self.effect_places.get(id) else {
            problems.error(
                RefKind::Effect,
                file,
                line,
                format!("{} ({id}) has no ::: params table to link to", effect.name),
            );
            return None;
        };
        let target = if anchor.is_empty() {
            page.clone()
        } else {
            format!("{page}#{anchor}")
        };
        let (href, _) = self.resolve_page(here, &target, problems, file, line)?;
        Some((href, effect.name.clone()))
    }

    /// Checks `[[action:name]]`: the page and anchor of its entry (`mcp-tracks#add_track`).
    pub fn resolve_action(
        &self,
        name: &str,
        problems: &mut Problems,
        file: &str,
        line: usize,
    ) -> Option<String> {
        match self.actions.get(name) {
            Some(page) => Some(format!("{page}#{name}")),
            None => {
                problems.error(
                    RefKind::Action,
                    file,
                    line,
                    format!("no MCP action is called \"{name}\""),
                );
                None
            }
        }
    }
}
