//! The MCP reference's examples (27.6): `docs/manual/mcp/examples/<area>.toml`, one file per
//! area of actions (`tracks.toml` for the actions in daw-core's `api/actions/tracks.rs`).
//!
//! ```toml
//! [[example]]
//! action = "add_track"
//! title = "Add a bus for the reverb"
//! notes = "Optional Markdown under the title."
//! setup = [
//!     { action = "set_tempo", input = '{"bpm": 90}' },
//! ]
//! input = '''
//! {"kind": "bus", "name": "Reverb 2"}
//! '''
//! result = '''
//! {"track": 9}
//! '''
//! ```
//!
//! Each example runs on a fresh headless session holding the fixture project (daw-core's
//! `api::actions::examples`, a quick-tier test): its `setup` calls first (each must succeed),
//! then the call, whose answer must match `result` (or, for an example of a refusal, `error`).
//! In both, `"*"` stands for any value and `<project>` for the fixture's folder; an object
//! with a `"..."` key may have more keys, and an array ending in `"..."` more items. With
//! `UPDATE_DOCS` set the runner writes what came back into `result`/`error` instead, keeping
//! those wildcards and every other byte of the file ([`rewrite`]). `shown_only = "why"` marks
//! an example that can't run headless (a screenshot, a device, a plug-in): it is shown, never
//! run, so its result is written by hand.
//!
//! The generator renders them under each action (its JSON request and result); this module
//! is also what the runner uses (the manual crate is a dev-dependency of daw-core).

use std::ops::Range;

use serde::Deserialize;
use serde_json::Value;
use toml::Spanned;

/// The examples' folder, in the manual's folder.
pub const DIR: &str = "mcp/examples";

/// In a result: any value at all.
pub const ANY: &str = "*";

/// In a result: an object's key (any value) allowing keys not listed, or an array's last item
/// allowing more items.
pub const MORE: &str = "...";

/// Where the fixture project's folder appears in inputs and results.
pub const PROJECT: &str = "<project>";

/// One call before the example's: its action and input (JSON).
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub action: String,
    pub input: Value,
}

/// An example as written.
#[derive(Debug, Clone)]
pub struct Example {
    pub action: String,
    pub title: String,
    /// Markdown shown under the title.
    pub notes: Option<String>,
    pub setup: Vec<Step>,
    /// The call's arguments (an object; `{}` when not given).
    pub input: Value,
    /// The arguments as written (the manual shows them so, in the author's order); empty when
    /// not given.
    pub input_text: String,
    /// The answer expected (a pattern, with [`ANY`] and [`MORE`]); `None` until written.
    pub result: Option<Value>,
    /// The refusal expected, for an example of one (`"*"`: any message).
    pub error: Option<String>,
    /// Why the example is only shown, never run (needs the app window, a device, ...).
    pub shown_only: Option<String>,
    /// 1-based line of its `action`.
    pub line: usize,
    spans: Spans,
}

#[derive(Debug, Clone, Default)]
struct Spans {
    /// The end of the last field this module knows the span of: where a missing `result` is
    /// written.
    end: usize,
    result: Option<Range<usize>>,
    error: Option<Range<usize>>,
}

/// An examples file: its area (the file's stem) and its examples in order.
#[derive(Debug, Clone)]
pub struct File {
    pub area: String,
    pub examples: Vec<Example>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default, rename = "example")]
    examples: Vec<RawExample>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExample {
    action: Spanned<String>,
    title: Spanned<String>,
    #[serde(default)]
    notes: Option<Spanned<String>>,
    #[serde(default)]
    setup: Option<Spanned<Vec<RawStep>>>,
    #[serde(default)]
    input: Option<Spanned<String>>,
    #[serde(default)]
    result: Option<Spanned<String>>,
    #[serde(default)]
    error: Option<Spanned<String>>,
    #[serde(default)]
    shown_only: Option<Spanned<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    action: String,
    #[serde(default)]
    input: Option<String>,
}

/// 1-based line of a byte offset.
fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

/// Arguments: a JSON object (empty text: `{}`).
fn arguments(text: &str) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    match serde_json::from_str::<Value>(text) {
        Ok(v @ Value::Object(_)) => Ok(v),
        Ok(other) => Err(format!(
            "an input is a JSON object of the action's arguments, not {other}"
        )),
        Err(e) => Err(format!("the input isn't JSON: {e}")),
    }
}

/// Reads an examples file. `name` is its file name (`tracks.toml`); errors come with their
/// line (0 for the whole file).
pub fn parse(name: &str, text: &str) -> Result<File, Vec<(usize, String)>> {
    let area = name.strip_suffix(".toml").unwrap_or(name).to_string();
    let raw: RawFile = toml::from_str(text).map_err(|e| {
        let line = e.span().map_or(0, |s| line_of(text, s.start));
        vec![(line, e.message().trim().to_string())]
    })?;
    let mut errors = Vec::new();
    let mut examples = Vec::new();
    for raw in raw.examples {
        let line = line_of(text, raw.action.span().start);
        let mut fail = |message: String| errors.push((line, message));
        let input = match &raw.input {
            Some(input) => arguments(input.get_ref()).unwrap_or_else(|e| {
                fail(format!("{}: {e}", raw.action.get_ref()));
                Value::Null
            }),
            None => Value::Object(Default::default()),
        };
        let mut setup = Vec::new();
        for step in raw.setup.iter().flat_map(|s| s.get_ref()) {
            match arguments(step.input.as_deref().unwrap_or("")) {
                Ok(input) => setup.push(Step {
                    action: step.action.clone(),
                    input,
                }),
                Err(e) => fail(format!("setup's {}: {e}", step.action)),
            }
        }
        let result = match &raw.result {
            Some(r) if r.get_ref().trim().is_empty() => None,
            Some(r) => match serde_json::from_str::<Value>(r.get_ref()) {
                Ok(v) => Some(v),
                Err(e) => {
                    fail(format!(
                        "{}: the result isn't JSON: {e}",
                        raw.action.get_ref()
                    ));
                    None
                }
            },
            None => None,
        };
        if raw.result.is_some() && raw.error.is_some() {
            fail(format!(
                "{}: an example has a result or an error, not both",
                raw.action.get_ref()
            ));
        }
        if raw.shown_only.is_some() && result.is_none() && raw.error.is_none() {
            fail(format!(
                "{}: an example only shown needs its result (or error) written by hand",
                raw.action.get_ref()
            ));
        }
        let mut end = raw.action.span().end.max(raw.title.span().end);
        if let Some(setup) = &raw.setup {
            end = end.max(setup.span().end);
        }
        for span in [
            &raw.input,
            &raw.result,
            &raw.error,
            &raw.notes,
            &raw.shown_only,
        ]
        .into_iter()
        .flatten()
        {
            end = end.max(span.span().end);
        }
        examples.push(Example {
            action: raw.action.get_ref().clone(),
            title: raw.title.get_ref().clone(),
            notes: raw.notes.as_ref().map(|n| n.get_ref().clone()),
            setup,
            input,
            input_text: raw
                .input
                .as_ref()
                .map(|i| i.get_ref().trim().to_string())
                .unwrap_or_default(),
            result,
            error: raw.error.as_ref().map(|e| e.get_ref().trim().to_string()),
            shown_only: raw.shown_only.as_ref().map(|s| s.get_ref().clone()),
            line,
            spans: Spans {
                end,
                result: raw.result.as_ref().map(Spanned::span),
                error: raw.error.as_ref().map(Spanned::span),
            },
        });
    }
    if errors.is_empty() {
        Ok(File { area, examples })
    } else {
        Err(errors)
    }
}

/// What a call answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Result(Value),
    Error(String),
}

/// Numbers match within a millionth (results hold levels and times in floating point).
fn same_number(a: &serde_json::Number, b: &serde_json::Number) -> bool {
    if let (Some(x), Some(y)) = (a.as_i64(), b.as_i64()) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (a.as_u64(), b.as_u64()) {
        return x == y;
    }
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1.0),
        _ => false,
    }
}

fn is_marker(value: &Value, marker: &str) -> bool {
    value.as_str() == Some(marker)
}

/// Whether `actual` matches the `pattern` ([`ANY`], [`MORE`]); the first difference, with
/// where it is, when not.
pub fn matches(pattern: &Value, actual: &Value) -> Result<(), String> {
    walk(pattern, actual, "")
}

fn at(path: &str) -> String {
    if path.is_empty() {
        "the result".to_string()
    } else {
        format!("`{path}`")
    }
}

fn walk(pattern: &Value, actual: &Value, path: &str) -> Result<(), String> {
    if is_marker(pattern, ANY) {
        return Ok(());
    }
    match (pattern, actual) {
        (Value::Object(p), Value::Object(a)) => {
            let open = p.contains_key(MORE);
            for (key, value) in p {
                if key == MORE {
                    continue;
                }
                let inner = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match a.get(key) {
                    Some(actual) => walk(value, actual, &inner)?,
                    None => return Err(format!("{} is missing", at(&inner))),
                }
            }
            if !open {
                if let Some(extra) = a.keys().find(|k| !p.contains_key(*k)) {
                    return Err(format!(
                        "{} has `{extra}`, which the example doesn't show (add it, or \
                         \"...\": \"*\" for keys not shown)",
                        at(path)
                    ));
                }
            }
            Ok(())
        }
        (Value::Array(p), Value::Array(a)) => {
            let open = p.last().is_some_and(|v| is_marker(v, MORE));
            let shown = if open { p.len() - 1 } else { p.len() };
            if (open && a.len() < shown) || (!open && a.len() != shown) {
                return Err(format!(
                    "{} has {} items, the example {}{}",
                    at(path),
                    a.len(),
                    shown,
                    if open { " or more" } else { "" }
                ));
            }
            for (n, (p, a)) in p.iter().zip(a).take(shown).enumerate() {
                walk(p, a, &format!("{path}[{n}]"))?;
            }
            Ok(())
        }
        (Value::Number(p), Value::Number(a)) if same_number(p, a) => Ok(()),
        (p, a) if p == a => Ok(()),
        (p, a) => Err(format!("{} is {a}, the example says {p}", at(path))),
    }
}

/// The example's expected outcome matched against what came back.
pub fn check(example: &Example, outcome: &Outcome) -> Result<(), String> {
    match (outcome, &example.error) {
        (Outcome::Error(message), Some(expected)) => {
            if expected == ANY || message == expected {
                Ok(())
            } else {
                Err(format!(
                    "refused with \"{message}\", the example says \"{expected}\""
                ))
            }
        }
        (Outcome::Error(message), None) => Err(format!("refused: {message}")),
        (Outcome::Result(value), Some(_)) => Err(format!(
            "the example shows a refusal, but it answered {}",
            pretty(value)
        )),
        (Outcome::Result(value), None) => match &example.result {
            Some(pattern) => matches(pattern, value),
            None => Err(
                "the example has no result yet: run the examples with UPDATE_DOCS=1 to \
                 write it, then read it"
                    .to_string(),
            ),
        },
    }
}

/// `actual` with the pattern's wildcards and elisions kept where it still fits: what
/// `UPDATE_DOCS` writes back.
pub fn refresh(pattern: Option<&Value>, actual: &Value) -> Value {
    match (pattern, actual) {
        (Some(p), _) if is_marker(p, ANY) => p.clone(),
        (Some(Value::Object(p)), Value::Object(a)) => {
            let mut out = serde_json::Map::new();
            if p.contains_key(MORE) {
                for (key, value) in p {
                    if key == MORE {
                        out.insert(key.clone(), value.clone());
                    } else if let Some(actual) = a.get(key) {
                        out.insert(key.clone(), refresh(Some(value), actual));
                    }
                }
            } else {
                for (key, value) in a {
                    out.insert(key.clone(), refresh(p.get(key), value));
                }
            }
            Value::Object(out)
        }
        (Some(Value::Array(p)), Value::Array(a)) => {
            let open = p.last().is_some_and(|v| is_marker(v, MORE));
            if open {
                let shown = p.len() - 1;
                let mut out: Vec<Value> = a
                    .iter()
                    .take(shown)
                    .enumerate()
                    .map(|(n, v)| refresh(p.get(n), v))
                    .collect();
                out.push(Value::String(MORE.to_string()));
                Value::Array(out)
            } else {
                Value::Array(
                    a.iter()
                        .enumerate()
                        .map(|(n, v)| refresh(p.get(n), v))
                        .collect(),
                )
            }
        }
        _ => actual.clone(),
    }
}

/// The file's text with each example's new result or error written in place (by its index
/// in the file), every other byte kept. An outcome of the other kind than the example
/// expects is not written (the runner fails on it).
pub fn rewrite(text: &str, file: &File, outcomes: &[(usize, Outcome)]) -> String {
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for (n, outcome) in outcomes {
        let Some(example) = file.examples.get(*n) else {
            continue;
        };
        match outcome {
            Outcome::Result(value) if example.error.is_none() => {
                let new = literal(&pretty(&refresh(example.result.as_ref(), value)));
                match &example.spans.result {
                    Some(span) => edits.push((span.clone(), new)),
                    None => {
                        let at = text[example.spans.end..]
                            .find('\n')
                            .map_or(text.len(), |i| example.spans.end + i + 1);
                        let lead = if at == text.len() && !text.ends_with('\n') {
                            "\n"
                        } else {
                            ""
                        };
                        edits.push((at..at, format!("{lead}result = {new}\n")));
                    }
                }
            }
            Outcome::Error(message) if example.error.is_some() => {
                let keep = example.error.as_deref() == Some(ANY);
                let new = if keep { ANY } else { message.as_str() };
                if let Some(span) = &example.spans.error {
                    edits.push((span.clone(), literal(new)));
                }
            }
            _ => {}
        }
    }
    edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    let mut out = text.to_string();
    for (span, new) in edits {
        out.replace_range(span, &new);
    }
    out
}

/// A TOML string holding `text`: a multi-line literal (nothing escaped) for JSON, a plain
/// literal for one line, a basic string when the text holds what those can't.
fn literal(text: &str) -> String {
    if text.contains('\n') {
        if !text.contains("'''") {
            return format!("'''\n{text}\n'''");
        }
    } else if !text.contains('\'') {
        return format!("'{text}'");
    }
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

/// JSON as the manual shows it and the files hold it: two-space indents, arrays of plain
/// values and short objects of plain values on one line.
pub fn pretty(value: &Value) -> String {
    let mut out = String::new();
    write_pretty(value, 0, &mut out);
    out
}

fn is_plain(value: &Value) -> bool {
    !matches!(value, Value::Array(_) | Value::Object(_))
        || value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(serde_json::Map::is_empty)
}

/// Short enough to sit on one line.
const INLINE: usize = 76;

fn write_pretty(value: &Value, indent: usize, out: &mut String) {
    let pad = |n: usize| "  ".repeat(n);
    match value {
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Array(items) if items.iter().all(is_plain) => {
            out.push('[');
            for (n, item) in items.iter().enumerate() {
                if n > 0 {
                    out.push_str(", ");
                }
                out.push_str(&item.to_string());
            }
            out.push(']');
        }
        Value::Object(map)
            if map
                .values()
                .all(|v| is_plain(v) || v.as_array().is_some_and(|a| a.iter().all(is_plain)))
                && value.to_string().len() <= INLINE =>
        {
            out.push('{');
            for (n, (key, item)) in map.iter().enumerate() {
                if n > 0 {
                    out.push_str(", ");
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push_str(": ");
                write_pretty(item, indent, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push_str("[\n");
            for (n, item) in items.iter().enumerate() {
                out.push_str(&pad(indent + 1));
                write_pretty(item, indent + 1, out);
                if n + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad(indent));
            out.push(']');
        }
        Value::Object(map) => {
            out.push_str("{\n");
            for (n, (key, item)) in map.iter().enumerate() {
                out.push_str(&pad(indent + 1));
                out.push_str(&Value::String(key.clone()).to_string());
                out.push_str(": ");
                write_pretty(item, indent + 1, out);
                if n + 1 < map.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad(indent));
            out.push('}');
        }
        plain => out.push_str(&plain.to_string()),
    }
}

/// Every string in `value` with `from` replaced by `to`: the fixture's folder and
/// [`PROJECT`], either way.
pub fn replace_in_strings(value: &Value, from: &str, to: &str) -> Value {
    match value {
        Value::String(s) => Value::String(s.replace(from, to)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| replace_in_strings(v, from, to))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), replace_in_strings(v, from, to)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FILE: &str = r#"# Track examples.

[[example]]
action = "add_track"
title = "Add a bus"
input = '''
{"kind": "bus", "name": "Reverb 2"}
'''
result = '''
{"track": 1}
'''

[[example]]
action = "set_track"
title = "Rename one"   # a comment kept
setup = [
    { action = "add_track", input = '{"kind": "audio"}' },
]
input = '{"track": 7, "name": "Bass"}'

[[example]]
action = "remove_track"
title = "A track that isn't there"
input = '{"track": 99}'
error = ''

[[example]]
action = "screenshot"
title = "The window"
shown_only = "needs the app window"
result = '"(a PNG image)"'
"#;

    #[test]
    fn a_file_reads_with_its_setup_inputs_and_patterns() {
        let file = parse("tracks.toml", FILE).unwrap();
        assert_eq!(file.area, "tracks");
        assert_eq!(file.examples.len(), 4);
        let first = &file.examples[0];
        assert_eq!(first.input, json!({"kind": "bus", "name": "Reverb 2"}));
        // Shown as written, in the author's order.
        assert_eq!(first.input_text, r#"{"kind": "bus", "name": "Reverb 2"}"#);
        assert_eq!(first.result, Some(json!({"track": 1})));
        assert_eq!(first.line, 4);
        let second = &file.examples[1];
        assert_eq!(
            second.setup,
            vec![Step {
                action: "add_track".into(),
                input: json!({"kind": "audio"})
            }]
        );
        assert_eq!(second.result, None);
        assert_eq!(file.examples[2].error.as_deref(), Some(""));
        assert_eq!(
            file.examples[3].shown_only.as_deref(),
            Some("needs the app window")
        );
    }

    #[test]
    fn bad_files_say_where() {
        let errors = parse(
            "x.toml",
            "[[example]]\naction = \"a\"\ntitle = \"t\"\ninput = '[1]'\n",
        )
        .unwrap_err();
        assert_eq!(errors[0].0, 2);
        assert!(errors[0].1.contains("JSON object"), "{errors:?}");
        let errors = parse("x.toml", "[[example]]\naction = \"a\"\ntitel = \"t\"\n").unwrap_err();
        assert!(errors[0].1.contains("titel"), "{errors:?}");
        let errors = parse(
            "x.toml",
            "[[example]]\naction = \"a\"\ntitle = \"t\"\nshown_only = \"why\"\n",
        )
        .unwrap_err();
        assert!(errors[0].1.contains("by hand"), "{errors:?}");
    }

    #[test]
    fn wildcards_and_elisions_match() {
        let actual = json!({"tracks": [{"id": 1, "name": "A"}, {"id": 2, "name": "B"}],
                            "path": "C:/x/Fixture/Fixture.oxide", "rate": 48000.0000001});
        assert!(matches(
            &json!({"tracks": [{"id": 1, "...": "*"}, "..."], "path": "*", "rate": 48000}),
            &actual
        )
        .is_ok());
        let e = matches(&json!({"tracks": "*", "path": "*"}), &actual).unwrap_err();
        assert!(e.contains("`rate`"), "{e}");
        let e = matches(
            &json!({"tracks": [{"id": 1, "name": "A"}], "path": "*", "rate": 48000}),
            &actual,
        )
        .unwrap_err();
        assert!(e.contains("2 items, the example 1"), "{e}");
        let e = matches(
            &json!({"tracks": [{"id": 1, "name": "Z"}, "..."], "path": "*", "rate": 48000}),
            &actual,
        )
        .unwrap_err();
        assert_eq!(e, "`tracks[0].name` is \"A\", the example says \"Z\"");
        assert!(matches(&json!("*"), &json!([1, 2])).is_ok());
    }

    #[test]
    fn refreshing_keeps_wildcards_and_elisions() {
        let actual = json!({"tracks": [{"id": 1, "name": "A", "gain": 0},
                                       {"id": 2, "name": "B", "gain": 0}],
                            "path": "C:/x", "new": true});
        let pattern = json!({"tracks": [{"id": 9, "...": "*"}, "..."], "path": "*"});
        assert_eq!(
            refresh(Some(&pattern), &actual),
            json!({"tracks": [{"id": 1, "...": "*"}, "..."], "path": "*", "new": true})
        );
        assert_eq!(refresh(None, &actual), actual);
    }

    #[test]
    fn rewriting_fills_results_and_errors_and_keeps_the_rest() {
        let file = parse("tracks.toml", FILE).unwrap();
        let text = rewrite(
            FILE,
            &file,
            &[
                (0, Outcome::Result(json!({"track": 12}))),
                (
                    1,
                    Outcome::Result(json!({"track": 7, "name": "Bass", "tags": [1, 2]})),
                ),
                (
                    2,
                    Outcome::Error("no track with id 99 (see get_project)".into()),
                ),
            ],
        );
        assert!(text.contains("result = '{\"track\": 12}'\n"), "{text}");
        assert!(text.contains(
            "input = '{\"track\": 7, \"name\": \"Bass\"}'\nresult = '{\"name\": \"Bass\", \
             \"tags\": [1, 2], \"track\": 7}'\n"
        ));
        // A result too long for a line is a multi-line literal.
        let long =
            json!({"tracks": [{"id": 1, "name": "Guitar L"}, {"id": 2, "name": "Guitar R"}]});
        let longer = rewrite(FILE, &file, &[(0, Outcome::Result(long.clone()))]);
        assert!(
            longer.contains("result = '''\n{\n  \"tracks\": [\n"),
            "{longer}"
        );
        assert_eq!(
            parse("tracks.toml", &longer).unwrap().examples[0].result,
            Some(long)
        );
        assert!(text.contains("error = 'no track with id 99 (see get_project)'"));
        assert!(text.contains("# a comment kept"));
        // A result written for an example whose setup comes last goes after its setup.
        let late = r#"[[example]]
action = "undo"
title = "t"
setup = [
    { action = "set_tempo", input = '{"bpm": 90}' },
]
"#;
        let file = parse("project.toml", late).unwrap();
        let written = rewrite(
            late,
            &file,
            &[(0, Outcome::Result(json!({"done": "Set tempo"})))],
        );
        assert_eq!(
            written,
            format!(
                "{late}result = '{{\"done\": \"Set tempo\"}}'
"
            )
        );
        assert!(text.starts_with("# Track examples.\n"));
        // What it wrote reads back as what came.
        let again = parse("tracks.toml", &text).unwrap();
        assert_eq!(again.examples[0].result, Some(json!({"track": 12})));
        assert_eq!(
            again.examples[2].error.as_deref(),
            Some("no track with id 99 (see get_project)")
        );
        // Wildcards written stay.
        let star = FILE.replace("{\"track\": 1}", "{\"track\": \"*\"}");
        let file = parse("tracks.toml", &star).unwrap();
        let text = rewrite(&star, &file, &[(0, Outcome::Result(json!({"track": 5})))]);
        assert!(text.contains("{\"track\": \"*\"}"));
    }

    #[test]
    fn checking_tells_results_from_refusals() {
        let file = parse("tracks.toml", FILE).unwrap();
        assert!(check(&file.examples[0], &Outcome::Result(json!({"track": 1}))).is_ok());
        let e = check(&file.examples[0], &Outcome::Error("no".into())).unwrap_err();
        assert_eq!(e, "refused: no");
        let e = check(&file.examples[1], &Outcome::Result(json!({}))).unwrap_err();
        assert!(e.contains("UPDATE_DOCS"), "{e}");
        let e = check(&file.examples[2], &Outcome::Result(json!({}))).unwrap_err();
        assert!(e.contains("shows a refusal"), "{e}");
    }

    #[test]
    fn json_is_pretty_but_compact() {
        let v = json!({"a": 1, "b": [1, 2, 3], "c": {"d": [{"e": 1}], "f": "g"}});
        assert_eq!(
            pretty(&v),
            "{\n  \"a\": 1,\n  \"b\": [1, 2, 3],\n  \"c\": {\n    \"d\": [\n      {\"e\": 1}\n    \
             ],\n    \"f\": \"g\"\n  }\n}"
        );
        assert_eq!(pretty(&json!({"track": 3})), "{\"track\": 3}");
        assert_eq!(
            replace_in_strings(&json!({"p": ["C:/f/x.wav"]}), "C:/f", PROJECT),
            json!({"p": ["<project>/x.wav"]})
        );
    }
}
