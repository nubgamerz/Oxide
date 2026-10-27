//! The MCP reference (27.6), generated from `mcp.json`: daw-core's action registry as
//! `api::describe` writes it (`mcp_docs_are_current`, which fails the quick tier while it is
//! stale; read here so this crate stays off daw-core's build, as [`crate::effects`] does).
//!
//! `manual.toml` places three generated entries:
//!
//! - `@mcp-reference`: `mcp-reference.html`, every action by area, a line each, linked to its
//!   entry, and what the badges mean;
//! - `@mcp-actions`: a page per area (`mcp-<area>.html`, `mcp-midi-clips.html` for
//!   `midi_clips`), each action an h2 anchored by its name (what `[[action:name]]` links to)
//!   with its badges, description, input and output tables ([`crate::schema`]) and its
//!   examples ([`crate::examples`]);
//! - `@mcp-prompts`: `mcp-prompts.html`, the prompts with their arguments and message.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use serde::Deserialize;
use serde_json::Value;

use crate::examples::{self, Example};
use crate::html::escape;
use crate::render::{Heading, Section};
use crate::schema;

const MCP: &str = include_str!("mcp.json");

/// The overview's page id.
pub const REFERENCE: &str = "mcp-reference";
/// The prompts' page id.
pub const PROMPTS: &str = "mcp-prompts";
/// The `manual.toml` entry that becomes a page per area.
pub const ACTIONS: &str = "mcp-actions";
/// The prose page the badges are explained on, linked from the generated pages when the
/// manual has it.
pub const HOW_ACTIONS_WORK: &str = "mcp-how-actions-work";

/// The overview's title.
pub const TITLE: &str = "MCP reference";
/// The prompts page's title.
pub const PROMPTS_TITLE: &str = "MCP prompts";

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Area {
    pub id: String,
    pub title: String,
    pub about: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Action {
    pub name: String,
    pub area: String,
    pub description: String,
    /// `read`, `edit`, `transport` or `destructive`.
    pub safety: String,
    /// What a call does to the undo history (daw-core's `api::Undo`): `always` (every
    /// successful call is one undo step), `when_changed` (one when the call changes the
    /// project, nothing when it changes nothing) or `never`.
    pub undo: String,
    /// Repeated calls on one object soon after each other are one undo step.
    pub merges: bool,
    /// Runs with no project open (the app's start screen).
    pub without_project: bool,
    /// Answered at once while a project opens (others wait).
    pub while_opening: bool,
    pub input: Value,
    /// `null` or any-value when the registry doesn't describe it.
    pub output: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PromptArg {
    pub name: String,
    pub description: String,
    pub required: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Prompt {
    pub name: String,
    pub title: String,
    pub description: String,
    pub arguments: Vec<PromptArg>,
    /// Its message with every argument left at its default.
    pub text: String,
}

/// Everything the MCP pages are made from.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Reference {
    pub instructions: String,
    pub areas: Vec<Area>,
    pub actions: Vec<Action>,
    pub prompts: Vec<Prompt>,
}

impl Reference {
    /// The registry's, from `mcp.json`.
    pub fn load() -> Reference {
        serde_json::from_str(MCP).expect("mcp.json is daw-core's own output")
    }

    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.name == name)
    }

    /// Each action's page id.
    pub fn pages(&self) -> BTreeMap<String, String> {
        self.actions
            .iter()
            .map(|a| (a.name.clone(), page_id(&a.area)))
            .collect()
    }
}

/// An area's page id: `mcp-` and the area with `-` for `_`.
pub fn page_id(area: &str) -> String {
    format!("mcp-{}", area.replace('_', "-"))
}

/// What the generated pages need to know about the rest of the manual.
pub struct Context<'a> {
    pub reference: &'a Reference,
    /// Each action's page id.
    pub pages: &'a BTreeMap<String, String>,
    /// The examples by action, in file order.
    pub examples: &'a BTreeMap<String, Vec<Example>>,
    /// The manual has the page explaining the badges.
    pub how_page: bool,
}

/// A generated page: its title, body, headings and sections for the search.
pub struct Generated {
    pub title: String,
    pub body: String,
    pub headings: Vec<Heading>,
    pub sections: Vec<Section>,
}

impl Context<'_> {
    /// A link to an action's entry, from page `here`.
    fn action_href(&self, name: &str, here: &str) -> Option<String> {
        let page = self.pages.get(name)?;
        Some(if page == here {
            format!("#{name}")
        } else {
            format!("{page}.html#{name}")
        })
    }

    /// Markdown-ish text (a description, an example's notes) as HTML: `code` spans that name
    /// an action and action names with an underscore become links; raw HTML is text. Returns
    /// the HTML and its plain words.
    fn prose(&self, text: &str, here: &str, paragraphs: bool) -> (String, String) {
        let mut html = String::new();
        let mut plain = String::new();
        for event in Parser::new(text) {
            match event {
                Event::Start(Tag::Paragraph) if !paragraphs => {}
                Event::End(TagEnd::Paragraph) if !paragraphs => html.push(' '),
                Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => {
                    plain.push_str(&t);
                    self.link_words(&t, here, &mut html);
                }
                Event::Code(c) => {
                    plain.push_str(&c);
                    let code = format!("<code>{}</code>", escape(&c));
                    match self.action_href(&c, here) {
                        Some(href) => {
                            let _ = write!(
                                html,
                                "<a class=\"ox-link ox-action\" href=\"{}\">{code}</a>",
                                escape(&href)
                            );
                        }
                        None => html.push_str(&code),
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    plain.push(' ');
                    html.push(' ');
                }
                other => {
                    if matches!(other, Event::End(TagEnd::Paragraph | TagEnd::Item)) {
                        plain.push(' ');
                    }
                    pulldown_cmark::html::push_html(&mut html, std::iter::once(other));
                }
            }
        }
        (html.trim().to_string(), plain)
    }

    /// Text with each action name that has an underscore (`get_project`, never `play`, a
    /// word too) linked.
    fn link_words(&self, text: &str, here: &str, html: &mut String) {
        let mut word = String::new();
        let flush = |word: &mut String, html: &mut String| {
            if word.contains('_') {
                if let Some(href) = self.action_href(word, here) {
                    let _ = write!(
                        html,
                        "<a class=\"ox-link ox-action\" href=\"{}\"><code>{}</code></a>",
                        escape(&href),
                        escape(word)
                    );
                    word.clear();
                    return;
                }
            }
            html.push_str(&escape(word));
            word.clear();
        };
        for c in text.chars() {
            if c.is_ascii_alphanumeric() || c == '_' {
                word.push(c);
            } else {
                flush(&mut word, html);
                html.push_str(&escape(&c.to_string()));
            }
        }
        flush(&mut word, html);
    }
}

/// A safety class's badge text and what it means.
fn safety(class: &str) -> (&'static str, &'static str) {
    match class {
        "read" => ("Read", "reads; changes nothing"),
        "edit" => (
            "Edit",
            "changes the project in a way you can see and reverse",
        ),
        "transport" => ("Transport", "starts, stops or moves playback or recording"),
        _ => ("Destructive", "deletes or overwrites something"),
    }
}

/// An action's badges.
fn badges(action: &Action) -> String {
    let mut out = String::from("<p class=\"ox-badges\">");
    let (name, what) = safety(&action.safety);
    let _ = write!(
        out,
        "<span class=\"ox-badge ox-badge-{}\" title=\"{}\">{name}</span>",
        escape(&action.safety),
        escape(what)
    );
    let mut badge = |text: &str, title: &str| {
        let _ = write!(
            out,
            " <span class=\"ox-badge\" title=\"{}\">{}</span>",
            escape(title),
            escape(text)
        );
    };
    let merges = if action.merges {
        " (repeats merge)"
    } else {
        ""
    };
    match action.undo.as_str() {
        "always" => badge(
            &format!("Undo step{merges}"),
            "Every call that succeeds is one step Edit > Undo takes back",
        ),
        "when_changed" => badge(
            &format!("Undo step when it changes the project{merges}"),
            "One step Edit > Undo takes back when the call changes the project; a call that \
             leaves it as it was records nothing",
        ),
        _ if action.safety != "read" => badge(
            "Not an undo step",
            "Edit > Undo doesn't take it back: what it changes isn't in the project's history",
        ),
        _ => {}
    }
    if action.safety != "read" {
        if action.without_project {
            badge(
                "Runs without a project",
                "Allowed on the start screen, with no project open",
            );
        } else {
            badge(
                "Needs a project",
                "Refused on the start screen, with no project open",
            );
        }
    }
    if action.while_opening {
        badge(
            "Answers while a project opens",
            "Other calls wait until the open is done",
        );
    }
    out.push_str("</p>\n");
    out
}

/// A schema's table (or a line when it has no fields), and the field names for the search.
fn schema_table(schema_value: &Value, heading: &str, input: bool, out: &mut String) -> String {
    let rows = schema::rows(schema_value);
    let mut names = String::new();
    let empty_object = rows.is_empty();
    if input && empty_object {
        let _ = writeln!(
            out,
            "<p class=\"ox-schema-none\"><strong>{heading}:</strong> none.</p>"
        );
        return names;
    }
    if !input && (schema_value.is_null() || schema::is_any(schema_value)) {
        let _ = writeln!(
            out,
            "<p class=\"ox-schema-none\"><strong>{heading}:</strong> not described here: \
             the description and the example say what comes back.</p>"
        );
        return names;
    }
    if !input && empty_object {
        let _ = writeln!(
            out,
            "<p class=\"ox-schema-none\"><strong>{heading}:</strong> an empty object.</p>"
        );
        return names;
    }
    if rows.len() == 1 && rows[0].path.is_empty() {
        let row = &rows[0];
        let what = if row.ty == "null" {
            "nothing (<code>null</code>)".to_string()
        } else {
            format!("{}.", escape(&row.ty))
        };
        let _ = writeln!(
            out,
            "<p class=\"ox-schema-none\"><strong>{heading}:</strong> {what} {}</p>",
            inline_code(&row.notes)
        );
        return names;
    }
    let _ = writeln!(
        out,
        "<table class=\"ox-params ox-schema\"><caption>{heading}</caption><thead><tr>\
         <th>Field</th><th>Type</th><th>What</th></tr></thead><tbody>"
    );
    for row in &rows {
        let path = if row.path.is_empty() {
            "(the result)".to_string()
        } else {
            row.path.clone()
        };
        let tag = if row.form {
            String::new()
        } else if input && !row.required {
            " <span class=\"ox-optional\">optional</span>".to_string()
        } else if !input && !row.required && !row.nullable {
            " <span class=\"ox-optional\">may be absent</span>".to_string()
        } else {
            String::new()
        };
        let class = if row.form {
            format!("ox-depth-{} ox-form", row.depth.min(6))
        } else {
            format!("ox-depth-{}", row.depth.min(6))
        };
        let _ = writeln!(
            out,
            "<tr class=\"{class}\"><td><code>{}</code>{tag}</td><td>{}</td><td>{}</td></tr>",
            escape(&path),
            escape(&row.ty),
            inline_code(&row.notes)
        );
        if let Some(leaf) = row.path.rsplit(['.', ']']).next() {
            if !leaf.is_empty() && !leaf.starts_with('<') && !names.contains(leaf) {
                names.push(' ');
                names.push_str(leaf);
            }
        }
    }
    out.push_str("</tbody></table>\n");
    names
}

/// Text with `code` in backticks (a schema's doc comments), escaped. An odd backtick is
/// left as it is.
fn inline_code(text: &str) -> String {
    let paired = text.matches('`').count().is_multiple_of(2);
    let mut out = String::new();
    for (n, part) in text.split('`').enumerate() {
        if n % 2 == 1 && paired {
            out.push_str("<code>");
            out.push_str(&escape(part));
            out.push_str("</code>");
        } else {
            if n > 0 && !paired {
                out.push('`');
            }
            out.push_str(&escape(part));
        }
    }
    out
}

/// A JSON block.
fn json_block(value: &Value) -> String {
    format!(
        "<pre class=\"ox-json\"><code>{}</code></pre>\n",
        escape(&examples::pretty(value))
    )
}

/// An example under its action.
fn example_html(ctx: &Context, example: &Example, here: &str, out: &mut String) -> String {
    let mut plain = example.title.clone();
    let _ = write!(
        out,
        "<div class=\"ox-example\">\n<p class=\"ox-example-head\">Example: {}",
        escape(&example.title)
    );
    if example.shown_only.is_some() {
        out.push_str(" <span class=\"ox-badge ox-badge-shown\">shown, not run</span>");
    }
    out.push_str("</p>\n");
    if let Some(notes) = &example.notes {
        let (html, text) = ctx.prose(notes, here, true);
        let _ = writeln!(out, "<div class=\"ox-example-notes\">{html}</div>");
        plain.push(' ');
        plain.push_str(&text);
    }
    if !example.setup.is_empty() {
        let mut calls = String::new();
        for step in &example.setup {
            let _ = writeln!(
                calls,
                "{} {}",
                step.action,
                serde_json::to_string(&step.input).unwrap_or_default()
            );
        }
        let _ = write!(
            out,
            "<p class=\"ox-example-label\">First, on the example project:</p>\n\
             <pre class=\"ox-json ox-setup\"><code>{}</code></pre>\n",
            escape(calls.trim_end())
        );
    }
    // The name before the arguments, as a client sends them; the arguments as written.
    let arguments = if example.input_text.is_empty() {
        "{}".to_string()
    } else {
        example.input_text.replace('\n', "\n  ")
    };
    let call = format!(
        "{{\n  \"name\": {},\n  \"arguments\": {arguments}\n}}",
        Value::String(example.action.clone())
    );
    let _ = write!(
        out,
        "<p class=\"ox-example-label\">The call (MCP <code>tools/call</code>):</p>\n\
         <pre class=\"ox-json\"><code>{}</code></pre>\n",
        escape(&call)
    );
    match (&example.result, &example.error) {
        (_, Some(error)) => {
            let _ = write!(
                out,
                "<p class=\"ox-example-label\">Refused, with:</p>\n<pre class=\"ox-json \
                 ox-refusal\"><code>{}</code></pre>\n",
                escape(error)
            );
        }
        (Some(result), None) => {
            let _ = write!(
                out,
                "<p class=\"ox-example-label\">The result:</p>\n{}",
                json_block(result)
            );
        }
        (None, None) => out.push_str(
            "<p class=\"ox-example-label\">The result: not written yet (the examples' \
             runner fills it).</p>\n",
        ),
    }
    if let Some(why) = &example.shown_only {
        let _ = writeln!(
            out,
            "<p class=\"ox-example-why\">Shown, not run: {}.</p>",
            escape(why.trim_end_matches('.'))
        );
    }
    out.push_str("</div>\n");
    plain
}

/// One line of an action's description: up to its first full stop.
fn summary(description: &str) -> &str {
    let end = description
        .match_indices(". ")
        .next()
        .map_or(description.len(), |(i, _)| i + 1);
    &description[..end]
}

/// The legend the generated pages share, linking the page that explains it when there.
fn legend(ctx: &Context) -> String {
    let more = if ctx.how_page {
        format!(
            " <a class=\"ox-link\" href=\"{HOW_ACTIONS_WORK}.html\">How actions work</a> says \
             more."
        )
    } else {
        String::new()
    };
    format!(
        "<p class=\"ox-legend\">Badges: <span class=\"ox-badge ox-badge-read\">Read</span> \
         changes nothing, <span class=\"ox-badge ox-badge-edit\">Edit</span> changes the \
         project, <span class=\"ox-badge ox-badge-transport\">Transport</span> plays, stops or \
         records, <span class=\"ox-badge ox-badge-destructive\">Destructive</span> deletes or \
         overwrites. In results, <code>\"*\"</code> stands for a value that differs from run to \
         run, <code>\"...\"</code> for more of the same, and <code>&lt;project&gt;</code> for \
         the example project's folder.{more}</p>\n"
    )
}

/// The page for one area.
pub fn area_page(ctx: &Context, area: &Area) -> Generated {
    let here = page_id(&area.id);
    let mut body = String::new();
    let actions: Vec<&Action> = ctx
        .reference
        .actions
        .iter()
        .filter(|a| a.area == area.id)
        .collect();
    let _ = write!(
        body,
        "<h1>{}</h1>\n<p>{} {} action{} here.</p>\n{}",
        escape(&area.title),
        escape(&area.about),
        actions.len(),
        if actions.len() == 1 { "" } else { "s" },
        legend(ctx)
    );
    // The page's top is found by its title only: its line on what the area covers is the
    // overview's section for it, so an area's page never outranks the guide's page on the
    // same words ("track output routing" is the mixer's question).
    let mut sections = vec![Section {
        anchor: String::new(),
        heading: String::new(),
        text: String::new(),
    }];
    let mut headings = Vec::new();
    for action in actions {
        let name = escape(&action.name);
        let _ = write!(
            body,
            "<h2 id=\"{name}\" class=\"ox-action-name\"><code>{name}</code> <a \
             class=\"ox-anchor\" href=\"#{name}\" aria-label=\"Link to this section\">#</a>\
             </h2>\n{}",
            badges(action)
        );
        let (description, mut text) = ctx.prose(&action.description, &here, true);
        let _ = writeln!(body, "<div class=\"ox-description\">{description}</div>");
        text.push_str(&schema_table(&action.input, "Input", true, &mut body));
        text.push_str(&schema_table(&action.output, "Result", false, &mut body));
        for example in ctx.examples.get(&action.name).into_iter().flatten() {
            text.push(' ');
            text.push_str(&example_html(ctx, example, &here, &mut body));
        }
        headings.push(Heading {
            level: 2,
            anchor: action.name.clone(),
            text: action.name.clone(),
        });
        sections.push(Section {
            anchor: action.name.clone(),
            heading: action.name.clone(),
            text: text.split_whitespace().collect::<Vec<_>>().join(" "),
        });
    }
    Generated {
        title: area.title.clone(),
        body,
        headings,
        sections,
    }
}

/// The overview: every action by area, a line each.
pub fn overview(ctx: &Context, areas_listed: bool) -> Generated {
    let reference = ctx.reference;
    let mut body = String::new();
    let _ = write!(
        body,
        "<h1>{TITLE}</h1>\n<p>Every action an AI agent can call through Oxide's MCP server: {} \
         in all, in {} areas, each with its own page. Ids come from <code>get_project</code>; \
         positions are samples at the project's sample rate unless an action says otherwise. \
         Oxide tells a client when it connects:</p>\n<blockquote><p>{}</p></blockquote>\n{}",
        reference.actions.len(),
        reference.areas.len(),
        escape(&reference.instructions),
        legend(ctx)
    );
    let mut sections = vec![Section {
        anchor: String::new(),
        heading: String::new(),
        text: format!(
            "Every action an AI agent can call through Oxide's MCP server. {}",
            reference.instructions
        ),
    }];
    let mut headings = Vec::new();
    for area in &reference.areas {
        let page = page_id(&area.id);
        let anchor = area.id.replace('_', "-");
        let title = if areas_listed {
            format!(
                "<a class=\"ox-link\" href=\"{page}.html\">{}</a>",
                escape(&area.title)
            )
        } else {
            escape(&area.title)
        };
        let _ = write!(
            body,
            "<h2 id=\"{anchor}\">{title} <a class=\"ox-anchor\" href=\"#{anchor}\" \
             aria-label=\"Link to this section\">#</a></h2>\n<p>{}</p>\n<table \
             class=\"ox-params ox-overview\"><thead><tr><th>Action</th><th>Kind</th>\
             <th>What it does</th></tr></thead><tbody>\n",
            escape(&area.about)
        );
        let mut names = String::new();
        for action in reference.actions.iter().filter(|a| a.area == area.id) {
            let code = format!("<code>{}</code>", escape(&action.name));
            let link = match ctx.action_href(&action.name, REFERENCE) {
                Some(href) if areas_listed => format!(
                    "<a class=\"ox-link ox-action\" href=\"{}\">{code}</a>",
                    escape(&href)
                ),
                _ => code,
            };
            let (line, _) = ctx.prose(summary(&action.description), REFERENCE, false);
            let _ = writeln!(
                body,
                "<tr><td>{link}</td><td><span class=\"ox-badge ox-badge-{}\">{}</span></td>\
                 <td>{line}</td></tr>",
                escape(&action.safety),
                safety(&action.safety).0
            );
            names.push(' ');
            names.push_str(&action.name);
        }
        body.push_str("</tbody></table>\n");
        headings.push(Heading {
            level: 2,
            anchor: anchor.clone(),
            text: area.title.clone(),
        });
        sections.push(Section {
            anchor,
            heading: area.title.clone(),
            text: format!("{}{names}", area.about),
        });
    }
    Generated {
        title: TITLE.to_string(),
        body,
        headings,
        sections,
    }
}

/// The prompts: each with its arguments and its message at the defaults.
pub fn prompts_page(ctx: &Context) -> Generated {
    let mut body = String::new();
    let _ = write!(
        body,
        "<h1>{PROMPTS_TITLE}</h1>\n<p>Guides a client can offer you (MCP \
         <code>prompts/get</code>): each is one message that walks an agent through the actions \
         for a job. In Claude, they show among the slash commands or the attachments \
         menu.</p>\n"
    );
    let mut sections = vec![Section {
        anchor: String::new(),
        heading: String::new(),
        text: "Guides a client can offer: each walks an agent through the actions for a job."
            .to_string(),
    }];
    let mut headings = Vec::new();
    for prompt in &ctx.reference.prompts {
        let name = escape(&prompt.name);
        let (description, mut text) = ctx.prose(&prompt.description, PROMPTS, true);
        let _ = write!(
            body,
            "<h2 id=\"{name}\"><code>{name}</code>: {} <a class=\"ox-anchor\" \
             href=\"#{name}\" aria-label=\"Link to this section\">#</a></h2>\n\
             <div class=\"ox-description\">{description}</div>\n",
            escape(&prompt.title)
        );
        if prompt.arguments.is_empty() {
            body.push_str("<p class=\"ox-schema-none\"><strong>Arguments:</strong> none.</p>\n");
        } else {
            body.push_str(
                "<table class=\"ox-params\"><caption>Arguments</caption><thead><tr>\
                 <th>Argument</th><th>What</th></tr></thead><tbody>\n",
            );
            for arg in &prompt.arguments {
                let (what, words) = ctx.prose(&arg.description, PROMPTS, false);
                let _ = writeln!(
                    body,
                    "<tr><td><code>{}</code>{}</td><td>{what}</td></tr>",
                    escape(&arg.name),
                    if arg.required {
                        ""
                    } else {
                        " <span class=\"ox-optional\">optional</span>"
                    }
                );
                text.push(' ');
                text.push_str(&arg.name);
                text.push(' ');
                text.push_str(&words);
            }
            body.push_str("</tbody></table>\n");
        }
        let (message, _) = ctx.prose(&prompt.text, PROMPTS, true);
        let _ = writeln!(
            body,
            "<details class=\"ox-prompt\"><summary>The message it sends (arguments left at \
             their defaults)</summary>\n<div class=\"ox-prompt-text\">{message}</div>\n\
             </details>"
        );
        headings.push(Heading {
            level: 2,
            anchor: prompt.name.clone(),
            text: prompt.title.clone(),
        });
        sections.push(Section {
            anchor: prompt.name.clone(),
            heading: format!("{}: {}", prompt.name, prompt.title),
            text,
        });
    }
    Generated {
        title: PROMPTS_TITLE.to_string(),
        body,
        headings,
        sections,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_s_actions_have_areas_and_schemas() {
        let reference = Reference::load();
        assert!(reference.actions.len() > 100, "{}", reference.actions.len());
        let add_track = reference.action("add_track").unwrap();
        assert_eq!(
            (add_track.area.as_str(), add_track.safety.as_str()),
            ("tracks", "edit")
        );
        assert_eq!(add_track.undo, "always");
        assert!(!add_track.without_project);
        assert_eq!(reference.action("set_track").unwrap().undo, "when_changed");
        assert_eq!(
            reference.action("set_plugin_parameter").unwrap().undo,
            "never"
        );
        assert!(reference.action("get_busy").unwrap().while_opening);
        assert!(reference.action("open_project").unwrap().without_project);
        assert_eq!(reference.action("save_project").unwrap().undo, "never");
        for action in &reference.actions {
            assert!(
                reference.areas.iter().any(|a| a.id == action.area),
                "{}",
                action.name
            );
        }
        assert_eq!(reference.prompts[0].name, "write_midi_part");
        assert!(reference.prompts[0].text.contains("describe_midi_clip"));
    }

    #[test]
    fn an_area_page_has_badges_tables_and_links() {
        let reference = Reference::load();
        let pages = reference.pages();
        let examples = BTreeMap::new();
        let ctx = Context {
            reference: &reference,
            pages: &pages,
            examples: &examples,
            how_page: true,
        };
        let tracks = reference.areas.iter().find(|a| a.id == "tracks").unwrap();
        let page = area_page(&ctx, tracks);
        assert!(page
            .body
            .contains("<h2 id=\"add_track\" class=\"ox-action-name\"><code>add_track</code>"));
        assert!(page.body.contains("<span class=\"ox-badge ox-badge-edit\""));
        assert!(page.body.contains("Undo step"));
        assert!(page.body.contains("<caption>Input</caption>"));
        // Another page's action, named in a description, is linked there.
        assert!(
            page.body
                .contains("href=\"mcp-project.html#get_project\"><code>get_project</code>"),
            "get_project linked"
        );
        assert!(page.body.contains("href=\"mcp-how-actions-work.html\""));
        let set_track_input = page
            .sections
            .iter()
            .find(|s| s.anchor == "set_track_input")
            .unwrap();
        assert!(
            set_track_input.text.contains("channel"),
            "field names searchable"
        );
        assert_eq!(page_id("midi_clips"), "mcp-midi-clips");
    }
}
