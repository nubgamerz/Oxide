//! Changing one member of a JSON file in place, leaving every other byte as it was.
//!
//! The clients' files belong to their users and to other programs: Claude Desktop's config has
//! the user's other servers and settings, and Claude Code's `.claude.json` is rewritten by the
//! CLI all the time. A parse and re-serialise would reorder keys (serde_json's maps sort them
//! unless `preserve_order` is on, and that feature would reach every crate in the workspace)
//! and reformat the whole file. So the text is edited instead: it is checked with serde_json,
//! the spans of the members are found by a small scanner, and only our member's bytes are
//! inserted, replaced or cut out. An insertion follows the surrounding indentation, and
//! removing what was inserted gives back the original bytes. Each edit is verified by parsing
//! the result and comparing every other value with the original's.

use serde_json::Value;

/// A member of an object: the byte offsets of its key's opening quote and of its value.
struct Member {
    key: String,
    start: usize,
    value: (usize, usize),
}

/// An object in the text: its braces and members.
struct Object {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

impl Object {
    fn find(&self, key: &str) -> Option<usize> {
        self.members.iter().position(|m| m.key == key)
    }
}

/// Walks text that serde_json has already accepted, so it only has to find where things are.
struct Scanner<'a> {
    text: &'a str,
    pos: usize,
}

impl Scanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Option<()> {
        (self.peek()? == byte).then(|| self.pos += 1)
    }

    /// Past a string (at its opening quote).
    fn string(&mut self) -> Option<()> {
        self.expect(b'"')?;
        loop {
            match self.peek()? {
                b'\\' => self.pos += 2,
                b'"' => {
                    self.pos += 1;
                    return Some(());
                }
                _ => self.pos += 1,
            }
        }
    }

    /// Past any value (at its first byte).
    fn value(&mut self) -> Option<()> {
        match self.peek()? {
            b'{' => self.object().map(|_| ()),
            b'[' => {
                self.pos += 1;
                self.space();
                if self.peek()? == b']' {
                    self.pos += 1;
                    return Some(());
                }
                loop {
                    self.value()?;
                    self.space();
                    match self.peek()? {
                        b',' => {
                            self.pos += 1;
                            self.space();
                        }
                        b']' => {
                            self.pos += 1;
                            return Some(());
                        }
                        _ => return None,
                    }
                }
            }
            b'"' => self.string(),
            _ => {
                // A number, true, false or null.
                let start = self.pos;
                while !matches!(
                    self.peek(),
                    None | Some(b',' | b']' | b'}' | b' ' | b'\t' | b'\r' | b'\n')
                ) {
                    self.pos += 1;
                }
                (self.pos > start).then_some(())
            }
        }
    }

    /// An object's members (at its `{`); afterwards just past its `}`.
    fn object(&mut self) -> Option<Object> {
        let open = self.pos;
        self.expect(b'{')?;
        let mut members = Vec::new();
        self.space();
        if self.peek()? == b'}' {
            self.pos += 1;
            return Some(Object {
                open,
                close: self.pos - 1,
                members,
            });
        }
        loop {
            self.space();
            let start = self.pos;
            self.string()?;
            let key = serde_json::from_str(&self.text[start..self.pos]).ok()?;
            self.space();
            self.expect(b':')?;
            self.space();
            let value_start = self.pos;
            self.value()?;
            members.push(Member {
                key,
                start,
                value: (value_start, self.pos),
            });
            self.space();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Some(Object {
                        open,
                        close: self.pos - 1,
                        members,
                    });
                }
                _ => return None,
            }
        }
    }
}

/// The text without a leading byte-order mark (serde_json refuses one), and the mark.
fn split_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    }
}

/// Parse a client's file. An empty file is an empty object (a client may leave one); the top
/// level must be an object.
pub fn parse(text: &str) -> Result<Value, String> {
    let (_, body) = split_bom(text);
    if body.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    let value: Value = serde_json::from_str(body).map_err(|e| {
        format!(
            "it isn't valid JSON (line {}, column {}: {e})",
            e.line(),
            e.column()
        )
    })?;
    if !value.is_object() {
        return Err("its top level isn't a JSON object".into());
    }
    Ok(value)
}

/// The top-level object of `body` (already parsed by [`parse`]).
fn top(body: &str) -> Result<Object, String> {
    let mut scanner = Scanner { text: body, pos: 0 };
    scanner.space();
    scanner
        .object()
        .ok_or_else(|| "its structure couldn't be followed".to_string())
}

/// The object that `top`'s member `parent` holds, if there is one.
fn child(body: &str, top: &Object, parent: &str) -> Result<Option<Object>, String> {
    let Some(i) = top.find(parent) else {
        return Ok(None);
    };
    let (start, _) = top.members[i].value;
    if body.as_bytes().get(start) != Some(&b'{') {
        return Err(format!("its \"{parent}\" isn't an object"));
    }
    let mut scanner = Scanner {
        text: body,
        pos: start,
    };
    scanner
        .object()
        .map(Some)
        .ok_or_else(|| "its structure couldn't be followed".to_string())
}

/// The whitespace at the start of the line holding `pos`.
fn line_indent(text: &str, pos: usize) -> &str {
    let line = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let end = text[line..]
        .find(|c: char| c != ' ' && c != '\t')
        .map_or(text.len(), |i| line + i);
    &text[line..end.min(pos)]
}

/// Whether `pos` is the first thing on its line.
fn starts_line(text: &str, pos: usize) -> bool {
    line_indent(text, pos).len() == pos - text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// The indentation step the file uses (two spaces when nothing says otherwise).
fn step(body: &str, top: &Object) -> String {
    top.members
        .first()
        .filter(|m| starts_line(body, m.start))
        .map(|m| line_indent(body, m.start).to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "  ".into())
}

/// `value` written at `indent`: pretty, each line after the first indented to match (or on
/// one line where the object it goes in is on one line).
fn render(value: &Value, indent: Option<&str>, step: &str) -> String {
    match indent {
        None => serde_json::to_string(value).unwrap_or_default(),
        Some(indent) => {
            let mut out = Vec::new();
            let formatter = serde_json::ser::PrettyFormatter::with_indent(step.as_bytes());
            let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
            serde::Serialize::serialize(value, &mut serializer).unwrap_or_default();
            let text = String::from_utf8(out).unwrap_or_default();
            text.replace('\n', &format!("\n{indent}"))
        }
    }
}

/// Insert `"name": value` into `object`, after its last member, matching its layout.
fn insert(body: &str, object: &Object, name: &str, value: &Value, step: &str) -> String {
    let key = serde_json::to_string(name).unwrap_or_default();
    let mut out = String::with_capacity(body.len() + 256);
    match object.members.last() {
        Some(last) => {
            let first = &object.members[0];
            let multiline = starts_line(body, first.start);
            out.push_str(&body[..last.value.1]);
            if multiline {
                let indent = line_indent(body, first.start);
                out.push_str(&format!(
                    ",\n{indent}{key}: {}",
                    render(value, Some(indent), step)
                ));
            } else {
                out.push_str(&format!(", {key}: {}", render(value, None, step)));
            }
            out.push_str(&body[last.value.1..]);
        }
        None => {
            let outer = line_indent(body, object.open);
            let indent = format!("{outer}{step}");
            out.push_str(&body[..object.open + 1]);
            out.push_str(&format!(
                "\n{indent}{key}: {}\n{outer}",
                render(value, Some(&indent), step)
            ));
            out.push_str(&body[object.close..]);
        }
    }
    out
}

/// `value` without `parent.name` (and without `parent` if `drop_parent`), for comparing
/// everything an edit must not have touched.
fn without(mut value: Value, parent: &str, name: &str, drop_parent: bool) -> Value {
    if let Some(top) = value.as_object_mut() {
        if drop_parent {
            top.remove(parent);
        } else if let Some(inner) = top.get_mut(parent).and_then(Value::as_object_mut) {
            inner.remove(name);
        }
    }
    value
}

/// Set `parent.name` to `value` in the JSON `text`: added at the end of `parent` (made if
/// missing) or put in place of the old value. Everything else stays byte for byte. Fails,
/// changing nothing, on text that isn't a JSON object or whose `parent` isn't an object.
pub fn set(text: &str, parent: &str, name: &str, value: &Value) -> Result<String, String> {
    let original = parse(text)?;
    let (bom, body) = split_bom(text);
    if body.trim().is_empty() {
        let mut fresh = serde_json::Map::new();
        let mut inner = serde_json::Map::new();
        inner.insert(name.into(), value.clone());
        fresh.insert(parent.into(), Value::Object(inner));
        return Ok(format!(
            "{bom}{}\n",
            render(&Value::Object(fresh), Some(""), "  ")
        ));
    }
    let top = top(body)?;
    let step = step(body, &top);
    let edited = match child(body, &top, parent)? {
        None => {
            let mut inner = serde_json::Map::new();
            inner.insert(name.into(), value.clone());
            insert(body, &top, parent, &Value::Object(inner), &step)
        }
        Some(object) => match object.find(name) {
            None => insert(body, &object, name, value, &step),
            Some(i) => {
                let member = &object.members[i];
                let indent =
                    starts_line(body, member.start).then(|| line_indent(body, member.start));
                format!(
                    "{}{}{}",
                    &body[..member.value.0],
                    render(value, indent, &step),
                    &body[member.value.1..]
                )
            }
        },
    };
    // The edit must have changed our member and nothing else.
    let result = parse(&edited).map_err(|e| format!("the edit went wrong ({e})"))?;
    let had_parent = original.get(parent).is_some();
    if result.get(parent).and_then(|p| p.get(name)) != Some(value)
        || without(result, parent, name, !had_parent) != without(original, parent, name, false)
    {
        return Err("the edit went wrong (other values changed)".into());
    }
    Ok(format!("{bom}{edited}"))
}

/// Remove `parent.name` from the JSON `text`, with the comma and line it brought: `None` if it
/// isn't there. Removing what [`set`] added gives back the text as it was.
pub fn remove(text: &str, parent: &str, name: &str) -> Result<Option<String>, String> {
    let original = parse(text)?;
    let (bom, body) = split_bom(text);
    if body.trim().is_empty() {
        return Ok(None);
    }
    let top = top(body)?;
    let Some(object) = child(body, &top, parent)? else {
        return Ok(None);
    };
    let Some(i) = object.find(name) else {
        return Ok(None);
    };
    let members = &object.members;
    let (from, to) = if members.len() == 1 {
        (object.open + 1, object.close)
    } else if i > 0 {
        (members[i - 1].value.1, members[i].value.1)
    } else {
        (members[0].start, members[1].start)
    };
    let edited = format!("{}{}", &body[..from], &body[to..]);
    let result = parse(&edited).map_err(|e| format!("the edit went wrong ({e})"))?;
    if result.get(parent).and_then(|p| p.get(name)).is_some()
        || without(result, parent, name, false) != without(original, parent, name, false)
    {
        return Err("the edit went wrong (other values changed)".into());
    }
    Ok(Some(format!("{bom}{edited}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DESKTOP: &str = r#"{
  "zeta": 1,
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "C:\\Users\\me"]
    },
    "blender": { "command": "uvx", "args": ["blender-mcp"] }
  },
  "alpha": {"nested": [1, 2, {"x": null}], "s": "a \"quoted\" } brace"},
  "globalShortcut": ""
}
"#;

    fn ours() -> Value {
        json!({"command": "D:\\Oxide\\oxide-mcp.exe", "args": ["--port", "7878"]})
    }

    #[test]
    fn adding_keeps_every_other_byte_and_removing_restores_the_file() {
        let added = set(DESKTOP, "mcpServers", "oxide", &ours()).unwrap();
        // Everything before our member is as it was, in order; ours is last, indented alike.
        // (serde_json writes our entry's own keys sorted.)
        let at = added
            .find(",\n    \"oxide\": {\n      \"args\"")
            .expect(&added);
        assert_eq!(&added[..at], &DESKTOP[..at]);
        assert!(added.contains("\n      \"args\": [\n        \"--port\",\n        \"7878\"\n      ],\n      \"command\": \"D:\\\\Oxide\\\\oxide-mcp.exe\"\n    }\n  },\n  \"alpha\""), "{added}");
        let value: Value = serde_json::from_str(&added).unwrap();
        assert_eq!(value["mcpServers"]["oxide"], ours());
        assert_eq!(value["zeta"], 1);
        assert_eq!(
            remove(&added, "mcpServers", "oxide").unwrap().unwrap(),
            DESKTOP
        );
    }

    #[test]
    fn replacing_changes_only_our_value() {
        let added = set(DESKTOP, "mcpServers", "oxide", &ours()).unwrap();
        let newer = json!({"command": "D:\\Oxide\\oxide-mcp.exe", "args": ["--port", "7899", "--token", "ab"]});
        let replaced = set(&added, "mcpServers", "oxide", &newer).unwrap();
        let value: Value = serde_json::from_str(&replaced).unwrap();
        assert_eq!(value["mcpServers"]["oxide"], newer);
        assert_eq!(
            remove(&replaced, "mcpServers", "oxide").unwrap().unwrap(),
            DESKTOP
        );
        // Setting the same value again changes nothing.
        assert_eq!(
            set(&replaced, "mcpServers", "oxide", &newer).unwrap(),
            replaced
        );
    }

    #[test]
    fn our_entry_first_or_alone_is_cut_cleanly() {
        let first = "{\n  \"mcpServers\": {\n    \"oxide\": {\"command\": \"x\"},\n    \"other\": {\"command\": \"y\"}\n  }\n}";
        assert_eq!(
            remove(first, "mcpServers", "oxide").unwrap().unwrap(),
            "{\n  \"mcpServers\": {\n    \"other\": {\"command\": \"y\"}\n  }\n}"
        );
        let alone = "{\"mcpServers\": {\"oxide\": {\"command\": \"x\"}}, \"k\": true}";
        assert_eq!(
            remove(alone, "mcpServers", "oxide").unwrap().unwrap(),
            "{\"mcpServers\": {}, \"k\": true}"
        );
        assert_eq!(remove(alone, "mcpServers", "nope").unwrap(), None);
        assert_eq!(remove("{\"k\": 1}", "mcpServers", "oxide").unwrap(), None);
    }

    #[test]
    fn a_missing_parent_or_file_is_made() {
        let text = "{\n    \"theme\": \"dark\"\n}\n";
        let added = set(text, "mcpServers", "oxide", &ours()).unwrap();
        assert!(added.starts_with("{\n    \"theme\": \"dark\",\n    \"mcpServers\": {\n        \"oxide\": {\n            \"args\""), "{added}");
        let value: Value = serde_json::from_str(&added).unwrap();
        assert_eq!(value["theme"], "dark");
        assert_eq!(value["mcpServers"]["oxide"], ours());

        for empty in ["", "  \n", "{}", "{ }"] {
            let made = set(empty, "mcpServers", "oxide", &ours()).unwrap();
            let value: Value = serde_json::from_str(&made).unwrap();
            assert_eq!(
                value,
                json!({"mcpServers": {"oxide": ours()}}),
                "{empty:?}: {made}"
            );
        }
        // An empty object of servers.
        let made = set("{\"mcpServers\": {}}", "mcpServers", "oxide", &ours()).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&made).unwrap()["mcpServers"]["oxide"],
            ours()
        );
        // On one line, it stays on one line.
        let inline = "{\"mcpServers\": {\"a\": {\"command\": \"b\"}}}";
        assert_eq!(
            set(inline, "mcpServers", "oxide", &json!({"command": "x"})).unwrap(),
            "{\"mcpServers\": {\"a\": {\"command\": \"b\"}, \"oxide\": {\"command\":\"x\"}}}"
        );
    }

    #[test]
    fn a_byte_order_mark_and_escaped_keys_survive() {
        let text =
            "\u{feff}{\"mcpServers\": {\"we\\\"ird\": {}, \"oxide\": {\"command\": \"old\"}}}";
        let replaced = set(text, "mcpServers", "oxide", &json!({"command": "new"})).unwrap();
        assert!(replaced.starts_with('\u{feff}'));
        assert!(replaced.contains("\"we\\\"ird\": {}"));
        assert!(
            replaced.contains("\"oxide\": {\"command\":\"new\"}"),
            "{replaced}"
        );
    }

    #[test]
    fn malformed_files_are_refused_with_the_reason() {
        let e = set(
            "{\"mcpServers\": {\"a\": 1,}",
            "mcpServers",
            "oxide",
            &ours(),
        )
        .unwrap_err();
        assert!(e.contains("isn't valid JSON (line 1"), "{e}");
        let e = set("[1, 2]", "mcpServers", "oxide", &ours()).unwrap_err();
        assert!(e.contains("top level isn't a JSON object"), "{e}");
        let e = set("{\"mcpServers\": []}", "mcpServers", "oxide", &ours()).unwrap_err();
        assert!(e.contains("\"mcpServers\" isn't an object"), "{e}");
        assert!(remove("{nope", "mcpServers", "oxide").is_err());
    }
}
