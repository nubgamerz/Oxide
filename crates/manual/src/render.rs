//! A page from Markdown to HTML, with the manual's extensions:
//!
//! - `[[Ctrl+C]]`: keys, drawn as `<kbd>`s;
//! - `[[page:id]]`, `[[page:id#anchor]]`, `[[page:#anchor]]`: a link to a page (or a heading
//!   on it), checked; its text is the page's title or the heading's unless given after a `|`
//!   (`[[page:id#anchor|this text]]`);
//! - `[[action:name]]`: an MCP action, checked against the registry, linked to its entry on
//!   its area's page of the MCP reference (`mcp-tracks.html#add_track`);
//! - `::: shot` figures (see [`crate::figure`]).
//!
//! Plain Markdown links to `other.md#anchor` and `#anchor` are checked and pointed at the
//! built pages too. Pages are read twice: [`scan`] learns every page's title and anchors, then
//! [`render`] resolves references against all of them.

use pulldown_cmark::{CowStr, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};

use crate::check::{Known, PageInfo, Problems, RefKind};
use crate::effects;
use crate::figure::{self, Figure, PLACEHOLDER};
use crate::html::{escape, slug};

/// A page's Markdown with its figures lifted out, read once and used by both passes.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub md: String,
    pub figures: Vec<Figure>,
}

/// A heading on a rendered page.
#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    pub level: u8,
    pub anchor: String,
    pub text: String,
}

/// A part of a page for the search index: the text under a heading (or before the first, with
/// an empty anchor).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Section {
    pub anchor: String,
    pub heading: String,
    pub text: String,
}

/// A rendered page.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub title: String,
    pub body: String,
    /// Headings below the title (h2-h3 make the page's own contents).
    pub headings: Vec<Heading>,
    pub sections: Vec<Section>,
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_WIKILINKS
        | Options::ENABLE_SMART_PUNCTUATION
}

/// Lifts the figures out of a page.
pub fn prepare(text: &str, file: &str, problems: &mut Problems) -> Prepared {
    let (md, figures) = figure::lift(text, file, problems);
    Prepared { md, figures }
}

/// 1-based line numbers from byte offsets.
struct Lines(Vec<usize>);

impl Lines {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Lines(starts)
    }

    fn at(&self, offset: usize) -> usize {
        self.0.partition_point(|&start| start <= offset)
    }
}

/// The first pass: the page's title and its headings' anchors. Reports a page without a
/// title, a second title, and anchors used twice.
pub fn scan(prepared: &Prepared, file: &str, problems: &mut Problems) -> PageInfo {
    let lines = Lines::new(&prepared.md);
    let mut info = PageInfo::default();
    let mut current: Option<(HeadingLevel, Option<String>, String, usize)> = None;
    let mut titles = 0;
    // The anchor of the last h2 or h3: where an effect's table is documented.
    let mut under = String::new();
    for (event, range) in Parser::new_ext(&prepared.md, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, id, .. }) => {
                current = Some((
                    level,
                    id.map(|i| i.to_string()),
                    String::new(),
                    lines.at(range.start),
                ));
            }
            Event::End(TagEnd::Heading(_)) => {
                let Some((level, id, text, line)) = current.take() else {
                    continue;
                };
                let text = text.trim().to_string();
                if level == HeadingLevel::H1 {
                    titles += 1;
                    if titles == 1 {
                        info.title = text;
                    } else {
                        problems.error(
                            RefKind::Syntax,
                            file,
                            line,
                            "a page has one title (# heading); use ## for its sections",
                        );
                    }
                    continue;
                }
                let anchor = id.unwrap_or_else(|| slug(&text));
                if level <= HeadingLevel::H3 {
                    under = anchor.clone();
                }
                if anchor.is_empty() {
                    problems.error(
                        RefKind::Syntax,
                        file,
                        line,
                        format!("heading \"{text}\" makes no anchor: give it one with {{#name}}"),
                    );
                } else {
                    match info.anchors.entry(anchor) {
                        std::collections::btree_map::Entry::Vacant(slot) => {
                            slot.insert(text);
                        }
                        std::collections::btree_map::Entry::Occupied(slot) => problems.error(
                            RefKind::DuplicateAnchor,
                            file,
                            line,
                            format!(
                                "anchor \"{}\" is used twice on this page: rename a heading                                  or give it its own with {{#name}}",
                                slot.key()
                            ),
                        ),
                    }
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, _, text, _)) = &mut current {
                    text.push_str(&t);
                }
            }
            Event::Html(html) if html.starts_with(effects::PLACEHOLDER) => {
                let id = html[effects::PLACEHOLDER.len()..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                info.effects
                    .push((id, under.clone(), lines.at(range.start)));
            }
            _ => {}
        }
    }
    if titles == 0 {
        problems.error(
            RefKind::Syntax,
            file,
            1,
            "the page has no title: start it with a # heading",
        );
    }
    info
}

/// What the reference resolver needs to know about the page being rendered.
struct Here<'a> {
    id: &'a str,
    file: &'a str,
    known: &'a Known,
}

/// An event, or a reference already turned into HTML (with its plain text for the search).
enum Item<'a> {
    Event(Event<'a>),
    Html(String, String),
}

/// Resolves wiki links and checks Markdown links; everything else passes through. `line` pins
/// every problem to one line (a figure's caption), else each event's own line is used.
fn transform<'a>(
    here: &Here,
    md: &'a str,
    line: Option<usize>,
    problems: &mut Problems,
) -> Vec<Item<'a>> {
    let lines = Lines::new(md);
    let mut out = Vec::new();
    let mut wiki: Option<(String, bool, Vec<Event<'a>>, usize)> = None;
    for (event, range) in Parser::new_ext(md, options()).into_offset_iter() {
        let at = line.unwrap_or_else(|| lines.at(range.start));
        if let Some((_, _, inner, _)) = &mut wiki {
            if matches!(event, Event::End(TagEnd::Link)) {
                let (dest, piped, inner, at) = wiki.take().expect("in a wiki link");
                let (html, plain) = wiki_link(here, &dest, piped.then_some(inner), at, problems);
                out.push(Item::Html(html, plain));
            } else {
                inner.push(event);
            }
            continue;
        }
        match event {
            Event::Start(Tag::Link {
                link_type: LinkType::WikiLink { has_pothole },
                dest_url,
                ..
            }) => wiki = Some((dest_url.trim().to_string(), has_pothole, Vec::new(), at)),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let dest_url =
                    markdown_link(here, &dest_url, at, problems).map_or(dest_url, CowStr::from);
                out.push(Item::Event(Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                })));
            }
            event => out.push(Item::Event(event)),
        }
    }
    out
}

/// `[[...]]`: keys, a page link or an action link.
fn wiki_link(
    here: &Here,
    dest: &str,
    inner: Option<Vec<Event>>,
    line: usize,
    problems: &mut Problems,
) -> (String, String) {
    let text = inner.map(|events| {
        let plain = plain_text(&events);
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, events.into_iter());
        (html, plain)
    });
    if let Some(target) = dest.strip_prefix("page:") {
        let target = target.trim();
        return match here
            .known
            .resolve_page(here.id, target, problems, here.file, line)
        {
            Some((href, default)) => {
                let (html, plain) = text.unwrap_or_else(|| (escape(&default), default));
                (
                    format!("<a class=\"ox-link\" href=\"{}\">{html}</a>", escape(&href)),
                    plain,
                )
            }
            None => broken(text, target),
        };
    }
    if let Some(name) = dest.strip_prefix("action:") {
        let name = name.trim();
        let Some(target) = here.known.resolve_action(name, problems, here.file, line) else {
            return broken(text, name);
        };
        let Some((href, _)) = here
            .known
            .resolve_page(here.id, &target, problems, here.file, line)
        else {
            return broken(text, name);
        };
        let (html, plain) =
            text.unwrap_or_else(|| (format!("<code>{}</code>", escape(name)), name.to_string()));
        return (
            format!(
                "<a class=\"ox-link ox-action\" href=\"{}\">{html}</a>",
                escape(&href)
            ),
            plain,
        );
    }
    if let Some(id) = dest.strip_prefix("effect:") {
        let id = id.trim();
        return match here
            .known
            .resolve_effect(here.id, id, problems, here.file, line)
        {
            Some((href, name)) => {
                let (html, plain) = text.unwrap_or_else(|| (escape(&name), name));
                (
                    format!("<a class=\"ox-link\" href=\"{}\">{html}</a>", escape(&href)),
                    plain,
                )
            }
            None => broken(text, id),
        };
    }
    if let Some((kind, _)) = dest.split_once(':') {
        if !kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase()) {
            problems.error(
                RefKind::Syntax,
                here.file,
                line,
                format!(
                    "unknown reference \"[[{dest}]]\": use [[page:...]], [[action:...]], \
                     [[effect:...]] or keys such as [[Ctrl+C]]"
                ),
            );
            return broken(text, dest);
        }
    }
    keys(dest, here, line, problems)
}

/// A reference that didn't resolve: its text, marked (the build fails anyway).
fn broken(text: Option<(String, String)>, target: &str) -> (String, String) {
    let (html, plain) = text.unwrap_or_else(|| (escape(target), target.to_string()));
    (format!("<span class=\"ox-broken\">{html}</span>"), plain)
}

/// `[[Ctrl+Shift+Z]]` → `<kbd>`s joined by `+`; `[[Ctrl++]]` is Ctrl and the plus key.
fn keys(dest: &str, here: &Here, line: usize, problems: &mut Problems) -> (String, String) {
    let mut keys = Vec::new();
    let mut current = String::new();
    for c in dest.chars() {
        if c == '+' && !current.trim().is_empty() {
            keys.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(c);
        }
    }
    if current.trim().is_empty() {
        problems.error(
            RefKind::Syntax,
            here.file,
            line,
            format!("keys \"[[{dest}]]\" end with a +: write [[Ctrl++]] for the plus key"),
        );
    } else {
        keys.push(current.trim().to_string());
    }
    let html = keys
        .iter()
        .map(|k| format!("<kbd>{}</kbd>", escape(k)))
        .collect::<Vec<_>>()
        .join("+");
    (
        format!("<span class=\"ox-keys\">{html}</span>"),
        keys.join("+"),
    )
}

/// A Markdown link to `page.md#anchor` or `#anchor`: checked and pointed at the built page.
/// `None` leaves the link as written (web links, files).
fn markdown_link(here: &Here, dest: &str, line: usize, problems: &mut Problems) -> Option<String> {
    if dest.contains("://") || dest.starts_with("mailto:") {
        return None;
    }
    let (path, anchor) = match dest.split_once('#') {
        Some((path, anchor)) => (path, Some(anchor)),
        None => (dest, None),
    };
    let id = if path.is_empty() {
        here.id
    } else {
        let name = path.rsplit('/').next().unwrap_or(path);
        name.strip_suffix(".md")?
    };
    let target = match anchor {
        Some(anchor) => format!("{id}#{anchor}"),
        None => id.to_string(),
    };
    here.known
        .resolve_page(here.id, &target, problems, here.file, line)
        .map(|(href, _)| href)
}

/// The words of some events, for the search and alt text.
fn plain_text(events: &[Event]) -> String {
    let mut text = String::new();
    for event in events {
        match event {
            Event::Text(t) | Event::Code(t) => text.push_str(t),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            _ => {}
        }
    }
    text
}

/// Inline Markdown (a caption) to HTML without its paragraph, and its plain text.
fn inline(here: &Here, md: &str, line: usize, problems: &mut Problems) -> (String, String) {
    let mut events = Vec::new();
    let mut plain = String::new();
    for item in transform(here, md, Some(line), problems) {
        match item {
            Item::Event(event) => {
                if let Event::Text(t) | Event::Code(t) = &event {
                    plain.push_str(t);
                }
                if !matches!(
                    event,
                    Event::Start(Tag::Paragraph) | Event::End(TagEnd::Paragraph)
                ) {
                    events.push(event);
                }
            }
            Item::Html(html, text) => {
                plain.push_str(&text);
                events.push(Event::InlineHtml(html.into()));
            }
        }
    }
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events.into_iter());
    (html.trim().to_string(), plain)
}

/// The second pass: the page's HTML, its headings and its sections for the search, with every
/// reference checked against `known`.
pub fn render(
    id: &str,
    file: &str,
    prepared: &Prepared,
    known: &Known,
    problems: &mut Problems,
) -> Rendered {
    let here = Here { id, file, known };
    let mut out: Vec<Event> = Vec::new();
    let mut title = String::new();
    let mut headings = Vec::new();
    let mut sections = vec![Section::default()];
    // In a heading: its level, explicit id, where its events start in `out`, its text.
    let mut heading: Option<(HeadingLevel, Option<String>, usize, String)> = None;
    let mut in_comment = false;
    for item in transform(&here, &prepared.md, None, problems) {
        let (event, plain) = match item {
            Item::Html(html, plain) => (Event::InlineHtml(html.into()), Some(plain)),
            Item::Event(event) => {
                let plain = match &event {
                    Event::Text(t) | Event::Code(t) => Some(t.to_string()),
                    Event::SoftBreak | Event::HardBreak => Some(" ".to_string()),
                    Event::End(
                        TagEnd::Paragraph
                        | TagEnd::Item
                        | TagEnd::TableCell
                        | TagEnd::CodeBlock
                        | TagEnd::BlockQuote(_),
                    ) => Some(" ".to_string()),
                    _ => None,
                };
                (event, plain)
            }
        };
        if let Some(plain) = plain {
            match &mut heading {
                Some((_, _, _, text)) => text.push_str(&plain),
                None => sections
                    .last_mut()
                    .expect("a section")
                    .text
                    .push_str(&plain),
            }
        }
        match event {
            Event::Html(ref html) if in_comment => {
                if html.contains("-->") {
                    in_comment = false;
                }
            }
            Event::Html(ref html) if html.starts_with(PLACEHOLDER) => {
                in_comment = !html.contains("-->");
                let number: usize = html[PLACEHOLDER.len()..]
                    .trim_end_matches(|c: char| !c.is_ascii_digit())
                    .parse()
                    .unwrap_or(usize::MAX);
                let Some(figure) = prepared.figures.get(number) else {
                    continue;
                };
                figure::check(figure, known, problems, file);
                let (caption, caption_text) = inline(&here, &figure.caption, figure.line, problems);
                let callouts: Vec<(Option<String>, String)> = figure
                    .highlights
                    .iter()
                    .filter_map(|h| {
                        let text = h.text.as_ref()?;
                        let (html, plain) = inline(&here, text, h.line, problems);
                        let section = &mut sections.last_mut().expect("a section").text;
                        section.push(' ');
                        section.push_str(&plain);
                        Some((h.label.clone(), html))
                    })
                    .collect();
                let section = &mut sections.last_mut().expect("a section").text;
                section.push(' ');
                section.push_str(&caption_text);
                out.push(Event::Html(
                    figure::html(figure, number, known, &caption, &caption_text, &callouts).into(),
                ));
            }
            Event::Html(ref html) if html.starts_with(effects::PLACEHOLDER) => {
                in_comment = !html.contains("-->");
                let id = html[effects::PLACEHOLDER.len()..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                match known.effects.get(id) {
                    Some(effect) => {
                        let (table, plain) = effects::table(effect, known.effect_rate);
                        let section = &mut sections.last_mut().expect("a section").text;
                        section.push(' ');
                        section.push_str(&plain);
                        out.push(Event::Html(table.into()));
                    }
                    // Reported by the build's check of every page's tables.
                    None => out.push(Event::Html(
                        format!("<p class=\"ox-broken\">{}</p>\n", escape(id)).into(),
                    )),
                }
            }
            Event::Start(Tag::Heading { level, id, .. }) => {
                heading = Some((level, id.map(|i| i.to_string()), out.len(), String::new()));
            }
            Event::End(TagEnd::Heading(_)) => {
                let Some((level, id, start, text)) = heading.take() else {
                    continue;
                };
                let inner: Vec<Event> = out.split_off(start);
                let mut html = String::new();
                pulldown_cmark::html::push_html(&mut html, inner.into_iter());
                let text = text.trim().to_string();
                if level == HeadingLevel::H1 {
                    if title.is_empty() {
                        title = text;
                    }
                    out.push(Event::Html(format!("<h1>{html}</h1>\n").into()));
                    continue;
                }
                let anchor = id.unwrap_or_else(|| slug(&text));
                let n = level as u8;
                out.push(Event::Html(
                    format!(
                        "<h{n} id=\"{a}\">{html} <a class=\"ox-anchor\" href=\"#{a}\" \
                         aria-label=\"Link to this section\">#</a></h{n}>\n",
                        a = escape(&anchor)
                    )
                    .into(),
                ));
                if n <= 3 {
                    sections.push(Section {
                        anchor: anchor.clone(),
                        heading: text.clone(),
                        text: String::new(),
                    });
                }
                headings.push(Heading {
                    level: n,
                    anchor,
                    text,
                });
            }
            event => out.push(event),
        }
    }
    let mut body = String::new();
    pulldown_cmark::html::push_html(&mut body, out.into_iter());
    for section in &mut sections {
        section.text = section
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
    }
    Rendered {
        title,
        body,
        headings,
        sections,
    }
}
