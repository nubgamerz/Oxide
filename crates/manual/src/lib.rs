//! Oxide's manual: Markdown pages in `docs/manual/` built to static HTML (dev plan phase 27).
//!
//! The pages are read from disk ([`Source::read`]) or made in memory (tests), checked and
//! rendered by [`build`], and written by [`write`]. Nothing here runs inside the app (it reads
//! the `search-index.json` this writes, through `manual-search`): it is a build step
//! (`check.ps1`'s quick tier runs `manual --check`; `installer.ps1` builds the manual it ships).
//!
//! - `docs/manual/manual.toml`: the table of contents (chapters, their pages in order). A page
//!   is a Markdown file listed by its path; its id is the file's stem (unique), and it is
//!   built to `<id>.html`. Entries starting with `@` are generated: `@mcp-reference`,
//!   `@mcp-actions` (a page per area) and `@mcp-prompts`, the MCP reference ([`mcp`]).
//! - `docs/manual/mcp/examples/<area>.toml`: the MCP reference's examples ([`examples`]),
//!   run by daw-core's examples test.
//! - `docs/manual/shots.toml`: the screenshots the `manual` tier takes (27.2, [`shots`]).
//! - The screenshots (`<name>.png`) and their marks (`<name>.json`), from `--shots DIR`.
//! - `docs/manual/README.md`: how to write pages (not a page itself).
//!
//! The output opens from disk (`file://`): classic scripts only, no `fetch`, relative links.

pub mod callout;
pub mod check;
pub mod effects;
pub mod examples;
pub mod figure;
pub mod html;
pub mod mcp;
pub mod readme_images;
pub mod render;
pub mod schema;
pub mod search;
pub mod shots;
mod template;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use check::{Known, PageInfo, Problem, Problems, RefKind, ShotInfo};
use render::{Prepared, Section};
use search::Index;

/// The version the manual documents: the workspace's (`[workspace.package] version`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The table of contents' file, in the manual's folder.
pub const TOC: &str = "manual.toml";

/// The authors' guide, in the manual's folder: not a page.
pub const AUTHORS: &str = "README.md";

/// A file left in the output folder, so a later build knows it may empty it.
const MARKER: &str = ".oxide-manual";

const CSS: &str = include_str!("../assets/manual.css");
const JS: &str = include_str!("../assets/manual.js");

/// A screenshot's files.
#[derive(Debug, Clone, Default)]
pub struct ShotSource {
    /// The PNG, when it is there.
    pub png: Option<PathBuf>,
    /// The marks JSON's text, when it is there.
    pub marks: Option<String>,
}

/// Everything the manual is built from.
#[derive(Debug, Clone, Default)]
pub struct Source {
    /// `manual.toml`'s text.
    pub toc: String,
    /// Each Markdown page: its path relative to the manual's folder (with `/`), and its text.
    pub pages: Vec<(String, String)>,
    /// Screenshots by name.
    pub shots: BTreeMap<String, ShotSource>,
    /// `shots.toml`'s text, when the manual has one.
    pub shot_list: Option<String>,
    /// The MCP examples: each file's name (`tracks.toml`) and text.
    pub examples: Vec<(String, String)>,
}

impl Source {
    /// Reads `dir` (the manual's folder: `manual.toml` and every `.md` under it but the
    /// authors' README) and the screenshots in `shots`.
    pub fn read(dir: &Path, shots: &Path) -> Result<Source> {
        let toc = fs::read_to_string(dir.join(TOC))
            .with_context(|| format!("reading {}", dir.join(TOC).display()))?;
        let mut pages = Vec::new();
        let mut folders = vec![dir.to_path_buf()];
        while let Some(folder) = folders.pop() {
            for entry in
                fs::read_dir(&folder).with_context(|| format!("reading {}", folder.display()))?
            {
                let path = entry?.path();
                if path.is_dir() {
                    if path != shots {
                        folders.push(path);
                    }
                    continue;
                }
                if path.extension().is_some_and(|e| e == "md") {
                    let relative = path
                        .strip_prefix(dir)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    if relative == AUTHORS {
                        continue;
                    }
                    let text = fs::read_to_string(&path)
                        .with_context(|| format!("reading {}", path.display()))?;
                    pages.push((relative, text));
                }
            }
        }
        pages.sort();
        let shot_list = match fs::read_to_string(dir.join(shots::LIST)) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(e).context(format!("reading {}", dir.join(shots::LIST).display()))
            }
        };
        let mut examples = Vec::new();
        let folder = dir.join(examples::DIR);
        if folder.is_dir() {
            for entry in
                fs::read_dir(&folder).with_context(|| format!("reading {}", folder.display()))?
            {
                let path = entry?.path();
                if path.extension().is_some_and(|e| e == "toml") {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let text = fs::read_to_string(&path)
                        .with_context(|| format!("reading {}", path.display()))?;
                    examples.push((name, text));
                }
            }
        }
        examples.sort();
        let mut source = Source {
            toc,
            pages,
            shots: BTreeMap::new(),
            shot_list,
            examples,
        };
        if shots.is_dir() {
            for entry in fs::read_dir(shots)? {
                let path = entry?.path();
                let (Some(stem), Some(extension)) = (path.file_stem(), path.extension()) else {
                    continue;
                };
                let shot = source
                    .shots
                    .entry(stem.to_string_lossy().into_owned())
                    .or_default();
                if extension == "png" {
                    shot.png = Some(path.clone());
                } else if extension == "json" {
                    shot.marks = Some(
                        fs::read_to_string(&path)
                            .with_context(|| format!("reading {}", path.display()))?,
                    );
                }
            }
        }
        Ok(source)
    }
}

/// How to build.
#[derive(Debug, Clone)]
pub struct Options {
    /// A missing screenshot is an error (from 27.2, when shots are generated).
    pub strict_shots: bool,
    /// The version in the footer.
    pub version: String,
    /// An MCP action without an example is an error, not a warning (27.6: `--require-examples`,
    /// once every action has one).
    pub require_examples: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            strict_shots: false,
            version: VERSION.to_string(),
            require_examples: false,
        }
    }
}

/// The MCP examples read and checked: each action's, in file order. A file is an area's
/// (`tracks.toml`), and holds only that area's actions.
fn read_examples(
    source: &Source,
    reference: &mcp::Reference,
    options: &Options,
    listed: bool,
    problems: &mut Problems,
) -> BTreeMap<String, Vec<examples::Example>> {
    let mut by_action: BTreeMap<String, Vec<examples::Example>> = BTreeMap::new();
    for (name, text) in &source.examples {
        let file = format!("{}/{name}", examples::DIR);
        let parsed = match examples::parse(name, text) {
            Ok(parsed) => parsed,
            Err(errors) => {
                for (line, message) in errors {
                    problems.error(RefKind::Example, &file, line, message);
                }
                continue;
            }
        };
        if !reference.areas.iter().any(|a| a.id == parsed.area) {
            problems.error(
                RefKind::Example,
                &file,
                0,
                format!(
                    "\"{}\" isn't an area of actions: name the file after one ({})",
                    parsed.area,
                    reference
                        .areas
                        .iter()
                        .map(|a| format!("{}.toml", a.id))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
            continue;
        }
        for example in parsed.examples {
            match reference.action(&example.action) {
                None => problems.error(
                    RefKind::Example,
                    &file,
                    example.line,
                    format!("no MCP action is called \"{}\"", example.action),
                ),
                Some(action) if action.area != parsed.area => problems.error(
                    RefKind::Example,
                    &file,
                    example.line,
                    format!(
                        "{} is a {} action: its examples go in {}/{}.toml",
                        action.name,
                        action.area,
                        examples::DIR,
                        action.area
                    ),
                ),
                Some(_) => {
                    for step in &example.setup {
                        if reference.action(&step.action).is_none() {
                            problems.error(
                                RefKind::Example,
                                &file,
                                example.line,
                                format!(
                                    "its setup calls \"{}\", which isn't an action",
                                    step.action
                                ),
                            );
                        }
                    }
                    if example.result.is_none()
                        && example.error.is_none()
                        && example.shown_only.is_none()
                    {
                        problems.warn(
                            RefKind::Example,
                            &file,
                            example.line,
                            format!(
                                "{}'s example \"{}\" has no result yet: run the examples \
                                 with UPDATE_DOCS=1 (docs/manual/README.md)",
                                example.action, example.title
                            ),
                        );
                    }
                    by_action
                        .entry(example.action.clone())
                        .or_default()
                        .push(example);
                }
            }
        }
    }
    // Every action has an example (once the manual has examples): an error per action once
    // required, else one warning.
    if listed && !source.examples.is_empty() {
        let missing: Vec<&str> = reference
            .actions
            .iter()
            .filter(|a| !by_action.contains_key(&a.name))
            .map(|a| a.name.as_str())
            .collect();
        if options.require_examples {
            for name in &missing {
                problems.error(
                    RefKind::Example,
                    examples::DIR,
                    0,
                    format!("the MCP action {name} has no example"),
                );
            }
        } else if !missing.is_empty() {
            problems.warn(
                RefKind::Example,
                examples::DIR,
                0,
                format!(
                    "{} of {} MCP actions have no example yet ({}{}); --require-examples \
                     makes each an error",
                    missing.len(),
                    reference.actions.len(),
                    missing[..missing.len().min(8)].join(", "),
                    if missing.len() > 8 { ", ..." } else { "" }
                ),
            );
        }
    }
    by_action
}

/// Where each built-in effect is documented (its `::: params` table), checked: an id that
/// isn't an effect, an effect documented twice, and, once a manual documents any effect, one
/// it doesn't.
fn place_effects(entries: &[Entry], source: &Source, known: &mut Known, problems: &mut Problems) {
    let all = effects::all();
    known.effect_rate = all.rate;
    known.effects = all.effects.into_iter().map(|e| (e.id.clone(), e)).collect();
    let mut first_file = None;
    for entry in entries {
        let Kind::Markdown(n) = entry.kind else {
            continue;
        };
        let file = &source.pages[n].0;
        for (id, anchor, line) in &known.pages[&entry.id].effects {
            first_file.get_or_insert_with(|| file.clone());
            if !known.effects.contains_key(id) {
                problems.error(
                    RefKind::Effect,
                    file,
                    *line,
                    format!("no built-in effect has the id \"{id}\""),
                );
            } else if let Some((page, _)) = known.effect_places.get(id) {
                problems.error(
                    RefKind::Effect,
                    file,
                    *line,
                    format!("{id} has its ::: params table twice (also on {page})"),
                );
            } else {
                known
                    .effect_places
                    .insert(id.clone(), (entry.id.clone(), anchor.clone()));
            }
        }
    }
    if let Some(file) = first_file {
        for (id, effect) in &known.effects {
            if !known.effect_places.contains_key(id) {
                problems.error(
                    RefKind::Effect,
                    &file,
                    0,
                    format!(
                        "the built-in effect {} ({id}) has no ::: params table: once a page \
                         documents one effect, every effect is documented",
                        effect.name
                    ),
                );
            }
        }
    }
}

/// A figure's image to make when the manual is written: its screenshot, cut and scaled.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageJob {
    pub source: PathBuf,
    /// The part shown (x, y, w, h); `None`: the whole screenshot (it has no marks file).
    pub crop: Option<[u32; 4]>,
    /// The size written; `None`: the crop's.
    pub size: Option<[u32; 2]>,
}

/// A built manual, not yet written.
#[derive(Debug, Default)]
pub struct Built {
    /// Output path (relative, with `/`) and contents.
    pub files: BTreeMap<String, String>,
    /// Output path and the image to make there.
    pub images: BTreeMap<String, ImageJob>,
    /// Errors and warnings, by file and line.
    pub problems: Vec<Problem>,
    pub index: Index,
    /// Page ids in the manual's order.
    pub pages: Vec<String>,
}

impl Built {
    pub fn errors(&self) -> usize {
        self.problems
            .iter()
            .filter(|p| p.severity == check::Severity::Error)
            .count()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TocFile {
    #[serde(rename = "chapter")]
    chapters: Vec<TocChapter>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TocChapter {
    title: String,
    pages: Vec<String>,
}

/// Where a page comes from.
enum Kind {
    /// The index into `Source::pages`.
    Markdown(usize),
    /// The MCP reference's overview.
    McpReference,
    /// An area's MCP actions: the index into the reference's areas.
    McpArea(usize),
    /// The MCP prompts.
    McpPrompts,
}

struct Entry {
    id: String,
    chapter: usize,
    kind: Kind,
}

/// The line of `manual.toml` naming `page`, for messages (0 when not found).
fn toc_line(toc: &str, page: &str) -> usize {
    let quoted = format!("\"{page}\"");
    toc.lines()
        .position(|l| l.contains(&quoted))
        .map_or(0, |n| n + 1)
}

fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".md").unwrap_or(name)
}

/// A page's id and title, for previous and next.
fn neighbour<'a>(entry: Option<&'a Entry>, known: &'a Known) -> Option<(&'a str, &'a str)> {
    entry.map(|e| (e.id.as_str(), known.pages[&e.id].title.as_str()))
}

/// Checks and renders the whole manual. Problems are collected in `Built::problems`; a build
/// with errors still renders what it can.
pub fn build(source: &Source, options: &Options, reference: &mcp::Reference) -> Built {
    let mut problems = Problems::default();
    let mut built = Built::default();

    // The table of contents.
    let toc: TocFile = match toml::from_str(&source.toc) {
        Ok(toc) => toc,
        Err(e) => {
            problems.error(RefKind::Contents, TOC, 0, format!("can't be read: {e}"));
            built.problems = problems.list;
            return built;
        }
    };
    let mut entries: Vec<Entry> = Vec::new();
    let mut ids = BTreeSet::new();
    let mut listed = BTreeSet::new();
    for (chapter, c) in toc.chapters.iter().enumerate() {
        for page in &c.pages {
            let line = toc_line(&source.toc, page);
            // `@mcp-actions` stands for a page per area: the first takes the entry's place,
            // the rest follow it.
            let mut more = Vec::new();
            let (id, kind) = if let Some(generated) = page.strip_prefix('@') {
                match generated {
                    mcp::REFERENCE => (generated.to_string(), Kind::McpReference),
                    mcp::PROMPTS => (generated.to_string(), Kind::McpPrompts),
                    mcp::ACTIONS if !reference.areas.is_empty() => {
                        more.extend(
                            reference
                                .areas
                                .iter()
                                .enumerate()
                                .skip(1)
                                .map(|(n, a)| (mcp::page_id(&a.id), Kind::McpArea(n))),
                        );
                        (mcp::page_id(&reference.areas[0].id), Kind::McpArea(0))
                    }
                    mcp::ACTIONS => continue,
                    _ => {
                        problems.error(
                            RefKind::Contents,
                            TOC,
                            line,
                            format!(
                                "\"{page}\" isn't a generated page (@{}, @{} or @{})",
                                mcp::REFERENCE,
                                mcp::ACTIONS,
                                mcp::PROMPTS
                            ),
                        );
                        continue;
                    }
                }
            } else {
                let path = page.replace('\\', "/");
                let Some(n) = source.pages.iter().position(|(p, _)| *p == path) else {
                    problems.error(
                        RefKind::Contents,
                        TOC,
                        line,
                        format!("lists \"{page}\", which isn't there"),
                    );
                    continue;
                };
                listed.insert(n);
                (stem(&path).to_string(), Kind::Markdown(n))
            };
            if !ids.insert(id.clone()) {
                problems.error(
                    RefKind::Contents,
                    TOC,
                    line,
                    format!(
                        "lists page \"{id}\" twice (pages are named by their file's name, \
                         which must differ across folders)"
                    ),
                );
                continue;
            }
            entries.push(Entry { id, chapter, kind });
            for (id, kind) in more {
                if !ids.insert(id.clone()) {
                    problems.error(
                        RefKind::Contents,
                        TOC,
                        line,
                        format!("the generated page \"{id}\" is listed twice"),
                    );
                    continue;
                }
                entries.push(Entry { id, chapter, kind });
            }
        }
    }
    for (n, (path, _)) in source.pages.iter().enumerate() {
        if !listed.contains(&n) {
            problems.error(
                RefKind::Contents,
                path,
                0,
                format!("the page isn't in {TOC}: add it to a chapter's pages"),
            );
        }
    }
    if entries.first().map(|e| e.id.as_str()) != Some("index") {
        problems.error(
            RefKind::Contents,
            TOC,
            0,
            "the first page must be index.md (it becomes index.html)",
        );
    }

    // The first pass: titles and anchors.
    let mut prepared: BTreeMap<usize, Prepared> = BTreeMap::new();
    let mut known = Known {
        actions: reference.pages(),
        strict_shots: options.strict_shots,
        ..Known::default()
    };
    if let Some(text) = &source.shot_list {
        match shots::ShotList::read(text) {
            Ok(list) => known.listed_shots = Some(list.shots.into_keys().collect()),
            Err(e) => problems.error(RefKind::Shot, shots::LIST, 0, format!("{e:#}")),
        }
    }
    for entry in &entries {
        let info = match entry.kind {
            Kind::Markdown(n) => {
                let (path, text) = &source.pages[n];
                let page = render::prepare(text, path, &mut problems);
                let info = render::scan(&page, path, &mut problems);
                prepared.insert(n, page);
                info
            }
            Kind::McpReference => PageInfo {
                title: mcp::TITLE.to_string(),
                anchors: reference
                    .areas
                    .iter()
                    .map(|a| (a.id.replace('_', "-"), a.title.clone()))
                    .collect(),
                ..PageInfo::default()
            },
            Kind::McpArea(n) => PageInfo {
                title: reference.areas[n].title.clone(),
                anchors: reference
                    .actions
                    .iter()
                    .filter(|a| a.area == reference.areas[n].id)
                    .map(|a| (a.name.clone(), a.name.clone()))
                    .collect(),
                ..PageInfo::default()
            },
            Kind::McpPrompts => PageInfo {
                title: mcp::PROMPTS_TITLE.to_string(),
                anchors: reference
                    .prompts
                    .iter()
                    .map(|p| (p.name.clone(), p.title.clone()))
                    .collect(),
                ..PageInfo::default()
            },
        };
        known.pages.insert(entry.id.clone(), info);
    }
    place_effects(&entries, source, &mut known, &mut problems);
    let areas_listed = entries.iter().any(|e| matches!(e.kind, Kind::McpArea(_)));
    let examples = read_examples(source, reference, options, areas_listed, &mut problems);
    let action_pages = known.actions.clone();
    let context = mcp::Context {
        reference,
        pages: &action_pages,
        examples: &examples,
        how_page: known.pages.contains_key(mcp::HOW_ACTIONS_WORK),
    };
    for (name, shot) in &source.shots {
        let marks = shot.marks.as_ref().and_then(|text| {
            serde_json::from_str::<figure::Marks>(text)
                .map_err(|e| {
                    problems.error(
                        RefKind::Shot,
                        &format!("shots/{name}.json"),
                        0,
                        format!("can't be read: {e}"),
                    )
                })
                .ok()
        });
        known.shots.insert(
            name.clone(),
            ShotInfo {
                image: shot.png.is_some(),
                marks,
            },
        );
    }

    // The second pass: every page rendered, references checked.
    let chapters: Vec<template::Chapter> = toc
        .chapters
        .iter()
        .enumerate()
        .map(|(n, c)| template::Chapter {
            title: c.title.clone(),
            pages: entries
                .iter()
                .filter(|e| e.chapter == n)
                .map(|e| (e.id.clone(), known.pages[&e.id].title.clone()))
                .collect(),
        })
        .collect();
    let mut index = Index {
        version: options.version.clone(),
        ..Index::default()
    };
    let mut figures = Vec::new();
    for (n, entry) in entries.iter().enumerate() {
        let (title, body, headings, sections) = match entry.kind {
            Kind::Markdown(page) => {
                let path = &source.pages[page].0;
                let prepared = &prepared[&page];
                figures.extend(prepared.figures.iter().cloned());
                let r = render::render(&entry.id, path, prepared, &known, &mut problems);
                (r.title, r.body, r.headings, r.sections)
            }
            Kind::McpReference | Kind::McpArea(_) | Kind::McpPrompts => {
                let page = match entry.kind {
                    Kind::McpArea(n) => mcp::area_page(&context, &reference.areas[n]),
                    Kind::McpPrompts => mcp::prompts_page(&context),
                    _ => mcp::overview(&context, areas_listed),
                };
                (page.title, page.body, page.headings, page.sections)
            }
        };
        let file = template::file_of(&entry.id);
        let page = index.add_page(&file, &title);
        for Section {
            anchor,
            heading,
            text,
        } in &sections
        {
            index.add_entry(page, anchor, heading, text);
        }
        let html = template::page(&template::Page {
            id: &entry.id,
            title: &title,
            body: &body,
            headings: &headings,
            chapters: &chapters,
            prev: neighbour(n.checked_sub(1).and_then(|p| entries.get(p)), &known),
            next: neighbour(entries.get(n + 1), &known),
            version: &options.version,
        });
        built.files.insert(file, html);
        built.pages.push(entry.id.clone());
    }
    // Each figure's image: its screenshot cut to its crop (one file per shot and crop).
    for figure in &figures {
        let Some(png) = source.shots.get(&figure.shot).and_then(|s| s.png.clone()) else {
            continue;
        };
        let job = match known.shots.get(&figure.shot).and_then(|s| s.marks.as_ref()) {
            Some(marks) => {
                let plan = figure::Plan::new(figure, marks);
                (
                    plan.file,
                    ImageJob {
                        source: png,
                        crop: Some(plan.crop),
                        size: Some(plan.size),
                    },
                )
            }
            None => (
                format!("shots/{}.png", figure.shot),
                ImageJob {
                    source: png,
                    crop: None,
                    size: None,
                },
            ),
        };
        built.images.insert(job.0, job.1);
    }
    built
        .files
        .insert("search-index.js".to_string(), index.script());
    // The same index for the app (`search_manual`, `open_manual`'s topic, 27.3).
    built
        .files
        .insert(search::JSON_FILE.to_string(), index.json());
    built
        .files
        .insert("assets/manual.css".to_string(), CSS.to_string());
    built
        .files
        .insert("assets/manual.js".to_string(), JS.to_string());
    built.index = index;
    let mut list = problems.list;
    list.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    built.problems = list;
    built
}

/// What [`write`] wrote.
#[derive(Debug, Default)]
pub struct Written {
    /// Each image's output path, its size in bytes and how it was encoded.
    pub images: Vec<(String, u64, shots::Encoding)>,
    /// Every file's bytes together, images included.
    pub total: u64,
}

impl Written {
    /// The images' bytes together.
    pub fn image_bytes(&self) -> u64 {
        self.images.iter().map(|i| i.1).sum()
    }
}

/// Writes a built manual to `out`, replacing an earlier build there, and makes the figures'
/// images (each screenshot read once). Refuses a folder that holds anything but an earlier
/// build.
pub fn write(built: &Built, out: &Path) -> Result<Written> {
    if out.exists() {
        let empty = fs::read_dir(out)?.next().is_none();
        if !empty && !out.join(MARKER).exists() {
            bail!(
                "{} isn't empty and isn't a manual built here before: not touching it",
                out.display()
            );
        }
        if !empty {
            fs::remove_dir_all(out).with_context(|| format!("emptying {}", out.display()))?;
        }
    }
    fs::create_dir_all(out)?;
    fs::write(
        out.join(MARKER),
        "Built by `cargo run -p manual`; emptied by the next build.\n",
    )?;
    let mut written = Written::default();
    for (path, text) in &built.files {
        let target = out.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, text).with_context(|| format!("writing {}", target.display()))?;
        written.total += text.len() as u64;
    }
    let mut read: BTreeMap<&Path, image::RgbImage> = BTreeMap::new();
    for (path, job) in &built.images {
        let target = out.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if !read.contains_key(job.source.as_path()) {
            read.insert(&job.source, shots::read(&job.source)?);
        }
        let source = &read[job.source.as_path()];
        let crop = job.crop.unwrap_or([0, 0, source.width(), source.height()]);
        let size = job.size.unwrap_or([crop[2], crop[3]]);
        let (bytes, encoding) = shots::image_file(source, crop, size)
            .with_context(|| format!("making {path} from {}", job.source.display()))?;
        fs::write(&target, &bytes).with_context(|| format!("writing {}", target.display()))?;
        written.total += bytes.len() as u64;
        written
            .images
            .push((path.clone(), bytes.len() as u64, encoding));
    }
    Ok(written)
}

#[cfg(test)]
mod tests;
