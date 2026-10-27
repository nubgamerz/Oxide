//! An action's JSON Schema (schemars 1: `$defs`, `$ref`, `anyOf` for `Option` of a type) as
//! the rows of a table a person reads (27.6): each field a row, nested objects and arrays
//! flattened into paths (`notes[].pitch`, `mixer.gain_db`), `$ref`s followed, a nullable type
//! shown as "or null", and a choice of forms (`{"type": "audio", ...}` or `{"type":
//! "midi", ...}`) as a row per form with its fields under it.

use serde_json::{Map, Value};

/// How deep nested fields go before the table stops (no schema here comes near it; a type
/// that contains itself would).
const MAX_DEPTH: usize = 8;

/// One row of a schema's table.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The field's path from the top (`notes[].pitch`); empty for the value itself (a result
    /// that isn't an object).
    pub path: String,
    /// Nesting depth, for indenting (0: a top-level field).
    pub depth: usize,
    /// "integer", "string or null", "array of objects", "one of 2 forms", "form".
    pub ty: String,
    /// Required in an input; always present in a result.
    pub required: bool,
    /// May be null (the type says "or null").
    pub nullable: bool,
    /// The field's description, then its allowed values, default and range.
    pub notes: String,
    /// A row that opens one of a field's forms (the rows under it, one deeper, are its
    /// fields).
    pub form: bool,
}

/// The table of `schema` (a whole schema, with its `$defs`): the fields of an object, or
/// one row for anything else.
pub fn rows(schema: &Value) -> Vec<Row> {
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let walker = Walker { defs: &defs };
    let mut out = Vec::new();
    let mut seen = Vec::new();
    let (inner, _) = walker.resolve(schema, &mut seen);
    if is_object(&inner) && inner.get("properties").is_some() {
        walker.fields(&inner, "", 0, &mut out, &mut seen);
    } else {
        walker.field(schema, "", 0, true, "", &mut out, &mut seen);
    }
    out
}

/// What a schema is, in a few words, for a summary ("an object", "nothing").
pub fn summary(schema: &Value) -> String {
    if schema.is_null() || schema == &Value::Bool(true) || is_any(schema) {
        return "not described".to_string();
    }
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let walker = Walker { defs: &defs };
    let mut seen = Vec::new();
    let (inner, nullable) = walker.resolve(schema, &mut seen);
    let ty = walker.type_text(&inner, &mut seen);
    if nullable {
        format!("{ty} or null")
    } else {
        ty
    }
}

/// A schema that says nothing (`serde_json::Value`'s: any value).
pub fn is_any(schema: &Value) -> bool {
    match schema {
        Value::Bool(true) => true,
        Value::Object(map) => map
            .keys()
            .all(|k| matches!(k.as_str(), "$schema" | "title" | "description")),
        _ => false,
    }
}

fn is_object(schema: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(t)) => t == "object",
        Some(Value::Array(ts)) => ts.iter().any(|t| t == "object"),
        None => schema.get("properties").is_some(),
        _ => false,
    }
}

struct Walker<'a> {
    defs: &'a Map<String, Value>,
}

impl Walker<'_> {
    /// Follows `$ref`s and unwraps `Option`'s `anyOf: [X, null]`: the schema meant, and
    /// whether null is allowed. A description beside a `$ref` is kept on the result.
    fn resolve(&self, schema: &Value, seen: &mut [String]) -> (Value, bool) {
        let mut schema = schema.clone();
        let mut nullable = false;
        for _ in 0..MAX_DEPTH {
            if let Some(Value::String(r)) = schema.get("$ref") {
                let name = r.rsplit('/').next().unwrap_or(r).to_string();
                let Some(def) = self.defs.get(&name) else {
                    break;
                };
                if seen.contains(&name) {
                    // A type inside itself: shown by name, not opened again.
                    let mut named = Map::new();
                    named.insert("type".into(), Value::String(name));
                    return (Value::Object(named), nullable);
                }
                let mut def = def.clone();
                // The field's own words win over the type's.
                for key in ["description", "default"] {
                    if let (Some(v), Value::Object(d)) = (schema.get(key), &mut def) {
                        d.insert(key.to_string(), v.clone());
                    }
                }
                schema = def;
                continue;
            }
            let any = schema
                .get("anyOf")
                .or_else(|| schema.get("oneOf"))
                .and_then(Value::as_array)
                .cloned();
            if let Some(any) = any {
                let (nulls, rest): (Vec<_>, Vec<_>) = any.into_iter().partition(|s| {
                    s.get("type").and_then(Value::as_str) == Some("null")
                        || s.get("const") == Some(&Value::Null)
                });
                if !nulls.is_empty() && rest.len() == 1 {
                    nullable = true;
                    let mut inner = rest.into_iter().next().expect("one");
                    for key in ["description", "default"] {
                        if let (Some(v), Value::Object(d)) = (schema.get(key), &mut inner) {
                            d.entry(key.to_string()).or_insert_with(|| v.clone());
                        }
                    }
                    schema = inner;
                    continue;
                }
            }
            break;
        }
        // `"type": ["integer", "null"]`: an Option of a plain type.
        if let Some(Value::Array(types)) = schema.get("type") {
            let rest: Vec<Value> = types.iter().filter(|t| *t != "null").cloned().collect();
            if rest.len() < types.len() {
                nullable = true;
                if let Value::Object(map) = &mut schema {
                    map.insert(
                        "type".into(),
                        if rest.len() == 1 {
                            rest[0].clone()
                        } else {
                            Value::Array(rest)
                        },
                    );
                }
            }
        }
        (schema, nullable)
    }

    /// The forms a schema offers (`anyOf`/`oneOf` that aren't just a nullable type, nor a
    /// choice of plain types such as an integer or a string), if any.
    fn forms(&self, schema: &Value) -> Option<Vec<Value>> {
        let forms = self.all_forms(schema)?;
        (self.plain_types(&forms).is_none()).then_some(forms)
    }

    /// A choice of plain types (`"integer or string"`), when every form is one.
    fn plain_types(&self, forms: &[Value]) -> Option<String> {
        let mut types: Vec<String> = Vec::new();
        for form in forms {
            let mut seen = Vec::new();
            let (form, _) = self.resolve(form, &mut seen);
            let ty = form.get("type").and_then(Value::as_str)?;
            if matches!(ty, "object" | "array")
                || form.get("const").is_some()
                || form.get("enum").is_some()
            {
                return None;
            }
            if !types.iter().any(|t| t == ty) {
                types.push(ty.to_string());
            }
        }
        Some(types.join(" or "))
    }

    /// Every form of an `anyOf`/`oneOf` but a null one.
    fn all_forms(&self, schema: &Value) -> Option<Vec<Value>> {
        let list = schema
            .get("oneOf")
            .or_else(|| schema.get("anyOf"))
            .and_then(Value::as_array)?;
        let forms: Vec<Value> = list
            .iter()
            .filter(|s| s.get("type").and_then(Value::as_str) != Some("null"))
            .cloned()
            .collect();
        (forms.len() > 1 || (forms.len() == 1 && list.len() == 1)).then_some(forms)
    }

    /// Plain words for a resolved schema's type.
    fn type_text(&self, schema: &Value, seen: &mut Vec<String>) -> String {
        if let Some(plain) = self
            .all_forms(schema)
            .and_then(|forms| self.plain_types(&forms))
        {
            return plain;
        }
        if let Some(forms) = self.forms(schema) {
            // Plain values only (an enum written as consts): name them in the notes.
            if forms
                .iter()
                .all(|f| f.get("const").is_some() || f.get("enum").is_some())
            {
                return scalar_kind(&forms);
            }
            return format!("one of {} forms", forms.len());
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            return scalar_kind(&values.to_vec());
        }
        if let Some(c) = schema.get("const") {
            return kind_of(c).to_string();
        }
        match schema.get("type") {
            Some(Value::String(t)) if t == "array" => match schema.get("items") {
                Some(items) => {
                    let (items, nullable) = self.resolve(items, seen);
                    let inner = self.type_text(&items, seen);
                    let inner = plural(&inner);
                    if nullable {
                        format!("array of {inner} (or null)")
                    } else {
                        format!("array of {inner}")
                    }
                }
                None => match schema.get("prefixItems").and_then(Value::as_array) {
                    Some(items) => format!("array of {} values", items.len()),
                    None => "array".to_string(),
                },
            },
            Some(Value::String(t)) if t == "object" => {
                if schema.get("properties").is_none() {
                    if let Some(values) = schema.get("additionalProperties") {
                        if values.is_object() {
                            let (values, _) = self.resolve(values, seen);
                            return format!("map of {}", plural(&self.type_text(&values, seen)));
                        }
                    }
                }
                "object".to_string()
            }
            Some(Value::String(t)) => t.clone(),
            Some(Value::Array(ts)) => ts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" or "),
            _ if is_any(schema) => "any".to_string(),
            _ => "any".to_string(),
        }
    }

    /// The rows for an object's properties, under `prefix`.
    fn fields(
        &self,
        schema: &Value,
        prefix: &str,
        depth: usize,
        out: &mut Vec<Row>,
        seen: &mut Vec<String>,
    ) {
        let required: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return;
        };
        // A form's tag (`"type": "audio"`) first, then the fields by name.
        let mut ordered: Vec<(&String, &Value)> = properties.iter().collect();
        ordered.sort_by_key(|(_, f)| f.get("const").is_none());
        for (name, field) in ordered {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            self.field(
                field,
                &path,
                depth,
                required.contains(&name.as_str()),
                "",
                out,
                seen,
            );
        }
    }

    /// One field's row, then whatever is inside it.
    #[allow(clippy::too_many_arguments)]
    fn field(
        &self,
        schema: &Value,
        path: &str,
        depth: usize,
        required: bool,
        lead: &str,
        out: &mut Vec<Row>,
        seen: &mut Vec<String>,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let (inner, nullable) = self.resolve(schema, seen);
        // Inside it, the type itself again is shown by name ([`Walker::resolve`]).
        let pushed = match ref_name(schema) {
            Some(name) => {
                seen.push(name);
                true
            }
            None => false,
        };
        let mut ty = self.type_text(&inner, seen);
        if nullable {
            ty.push_str(" or null");
        }
        let notes = notes(&inner, lead);
        out.push(Row {
            path: path.to_string(),
            depth,
            ty,
            required: required && !nullable,
            nullable,
            notes,
            form: false,
        });
        self.inside(&inner, path, depth, out, seen);
        if pushed {
            seen.pop();
        }
    }

    /// The rows under a field: an object's fields, an array's items, a map's values, the
    /// forms of a choice.
    fn inside(
        &self,
        inner: &Value,
        path: &str,
        depth: usize,
        out: &mut Vec<Row>,
        seen: &mut Vec<String>,
    ) {
        if let Some(forms) = self.forms(inner) {
            if forms
                .iter()
                .all(|f| f.get("const").is_some() || f.get("enum").is_some())
            {
                return;
            }
            for (n, form) in forms.iter().enumerate() {
                let (form, _) = self.resolve(form, seen);
                let tag = form_tag(&form);
                out.push(Row {
                    path: path.to_string(),
                    depth: depth + 1,
                    ty: match &tag {
                        Some(_) => "form".to_string(),
                        None => self.type_text(&form, seen),
                    },
                    required: false,
                    nullable: false,
                    notes: {
                        let mut text = format!("Form {}", n + 1);
                        if let Some(tag) = &tag {
                            text.push_str(&format!(": {tag}"));
                        }
                        let own = notes(&form, "");
                        if !own.is_empty() {
                            text.push_str(". ");
                            text.push_str(&own);
                        }
                        text
                    },
                    form: true,
                });
                if is_object(&form) {
                    self.fields(&form, path, depth + 2, out, seen);
                } else {
                    self.inside(&form, path, depth + 1, out, seen);
                }
            }
            return;
        }
        if is_object(inner) {
            if inner.get("properties").is_some() {
                self.fields(inner, path, depth + 1, out, seen);
            } else if let Some(values) = inner.get("additionalProperties") {
                if values.is_object() {
                    let key = format!("{path}.<key>");
                    self.field(values, &key, depth + 1, true, "", out, seen);
                }
            }
            return;
        }
        if inner.get("type").and_then(Value::as_str) == Some("array") {
            if let Some(items) = inner.get("items") {
                let (resolved, _) = self.resolve(items, seen);
                let item_path = format!("{path}[]");
                // Items that are plain values say all in the array's row.
                if is_object(&resolved) && resolved.get("properties").is_some() {
                    let pushed = match items.get("$ref").and_then(Value::as_str) {
                        Some(r) => {
                            seen.push(r.rsplit('/').next().unwrap_or(r).to_string());
                            true
                        }
                        None => false,
                    };
                    self.fields(&resolved, &item_path, depth + 1, out, seen);
                    if pushed {
                        seen.pop();
                    }
                } else if self.forms(&resolved).is_some()
                    || resolved.get("type").and_then(Value::as_str) == Some("array")
                {
                    self.field(items, &item_path, depth + 1, true, "", out, seen);
                }
            }
        }
    }
}

/// The type a `$ref` names.
fn ref_name(schema: &Value) -> Option<String> {
    let r = schema.get("$ref")?.as_str()?;
    Some(r.rsplit('/').next().unwrap_or(r).to_string())
}

/// A form's distinguishing value: `type = "audio"` for an internally tagged form, or its one
/// key (`Plugin`) for an externally tagged one.
fn form_tag(form: &Value) -> Option<String> {
    let properties = form.get("properties").and_then(Value::as_object)?;
    for (key, schema) in properties {
        if let Some(c) = schema.get("const") {
            return Some(format!("{key} = {c}"));
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            if values.len() == 1 {
                return Some(format!("{key} = {}", values[0]));
            }
        }
    }
    let required = form.get("required").and_then(Value::as_array)?;
    if properties.len() == 1 && required.len() == 1 {
        return properties
            .keys()
            .next()
            .map(|k| format!("{{\"{k}\": ...}}"));
    }
    None
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Number(n) if n.is_f64() => "number",
        Value::Number(_) => "integer",
        Value::Bool(_) => "boolean",
        Value::Null => "null",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The type of a list of allowed values (all strings: "string").
fn scalar_kind(values: &[Value]) -> String {
    let mut kinds: Vec<&str> = Vec::new();
    for v in values {
        let all: Vec<Value> = match (v.get("const"), v.get("enum")) {
            (Some(c), _) => vec![c.clone()],
            (None, Some(Value::Array(e))) => e.clone(),
            _ => vec![v.clone()],
        };
        for v in all {
            let k = kind_of(&v);
            if !kinds.contains(&k) {
                kinds.push(k);
            }
        }
    }
    kinds.join(" or ")
}

/// "object" → "objects", "string or null" → "strings or nulls" is too clever: plural of the
/// first word only.
fn plural(ty: &str) -> String {
    match ty.split_once(' ') {
        Some((first, rest)) if !first.ends_with('s') => format!("{first}s {rest}"),
        Some(_) => ty.to_string(),
        None if ty.ends_with('s') => ty.to_string(),
        None => format!("{ty}s"),
    }
}

/// A value as the notes show it: strings quoted, numbers as they are.
fn shown(value: &Value) -> String {
    match value {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

/// A field's description, then its values, default and range, as sentences.
fn notes(schema: &Value, lead: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !lead.is_empty() {
        parts.push(lead.to_string());
    }
    if let Some(d) = schema.get("description").and_then(Value::as_str) {
        let d = d.trim();
        if !d.is_empty() {
            parts.push(sentence(d));
        }
    }
    let mut values: Vec<Value> = Vec::new();
    if let Some(e) = schema.get("enum").and_then(Value::as_array) {
        values.extend(e.iter().cloned());
    }
    if let Some(list) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        if list
            .iter()
            .all(|f| f.get("const").is_some() || f.get("enum").is_some())
        {
            // Each value with its own description, when it has one.
            let mut described = Vec::new();
            for f in list {
                let these: Vec<Value> = match (f.get("const"), f.get("enum")) {
                    (Some(c), _) => vec![c.clone()],
                    (None, Some(Value::Array(e))) => e.clone(),
                    _ => Vec::new(),
                };
                match f.get("description").and_then(Value::as_str) {
                    Some(d) if these.len() == 1 => described.push(format!(
                        "{} ({})",
                        shown(&these[0]),
                        d.trim_end_matches('.')
                    )),
                    _ => described.extend(these.iter().map(shown)),
                }
            }
            if !described.is_empty() {
                parts.push(format!("One of: {}.", described.join(", ")));
            }
        }
    }
    values.retain(|v| !v.is_null());
    if !values.is_empty() {
        parts.push(format!(
            "One of: {}.",
            values.iter().map(shown).collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(c) = schema.get("const") {
        parts.push(format!("Always {}.", shown(c)));
    }
    // An integer type's own limits (a u8's 0 to 255) say nothing a reader needs.
    let format = schema.get("format").and_then(Value::as_str).unwrap_or("");
    let integer = format.starts_with("uint") || format.starts_with("int");
    let own_limit = |v: &Value| {
        integer
            && v.as_f64().is_some_and(|x| {
                x == 0.0
                    || [8, 16, 32, 64].iter().any(|&bits| {
                        let half = 2f64.powi(bits - 1);
                        x == 2f64.powi(bits) - 1.0 || x == half - 1.0 || x == -half
                    })
            })
    };
    let min = schema.get("minimum").filter(|m| !own_limit(m));
    let max = schema.get("maximum").filter(|m| !own_limit(m));
    match (min, max) {
        (Some(a), Some(b)) => parts.push(format!("From {a} to {b}.")),
        (Some(a), None) => parts.push(format!("At least {a}.")),
        (None, Some(b)) => parts.push(format!("At most {b}.")),
        (None, None) => {}
    }
    if let Some(n) = schema.get("minItems") {
        parts.push(format!("At least {n} items."));
    }
    if let Some(d) = schema.get("default") {
        let empty = d.is_null() || d.as_array().is_some_and(Vec::is_empty);
        if !empty {
            parts.push(format!("Default: {}.", shown(d)));
        }
    }
    parts.join(" ")
}

/// Ends with a full stop.
fn sentence(text: &str) -> String {
    if text.ends_with(['.', '!', '?', ')', ':']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_objects_arrays_options_and_refs_flatten_into_paths() {
        let schema = json!({
            "type": "object",
            "properties": {
                "clip": {"type": "integer", "format": "uint32", "minimum": 0},
                "notes": {"type": "array", "items": {"$ref": "#/$defs/NewNote"}},
                "gain_db": {"type": ["number", "null"], "default": null,
                            "description": "Level in dB", "minimum": -60, "maximum": 12},
                "fade": {"anyOf": [{"$ref": "#/$defs/Shape"}, {"type": "null"}],
                         "description": "The fade's shape."}
            },
            "required": ["clip", "notes"],
            "$defs": {
                "NewNote": {"type": "object", "properties": {
                    "pitch": {"description": "MIDI note or a name (\"C4\")",
                              "anyOf": [{"type": "integer"}, {"type": "string"}]},
                    "velocity": {"type": "integer", "default": 100}
                }, "required": ["pitch"]},
                "Shape": {"type": "string", "enum": ["linear", "s_curve"]}
            }
        });
        let rows = rows(&schema);
        let paths: Vec<(&str, &str, bool)> = rows
            .iter()
            .map(|r| (r.path.as_str(), r.ty.as_str(), r.required))
            .collect();
        assert_eq!(
            paths,
            vec![
                ("clip", "integer", true),
                ("fade", "string or null", false),
                ("gain_db", "number or null", false),
                ("notes", "array of objects", true),
                ("notes[].pitch", "integer or string", true),
                ("notes[].velocity", "integer", false),
            ]
        );
        assert_eq!(
            rows[1].notes,
            "The fade's shape. One of: \"linear\", \"s_curve\"."
        );
        assert_eq!(rows[2].notes, "Level in dB. From -60 to 12.");
        assert_eq!(rows[5].notes, "Default: 100.");
        assert_eq!(rows[4].depth, 1);
        assert_eq!(rows[0].notes, "", "an unsigned minimum 0 says nothing");
    }

    #[test]
    fn tagged_forms_are_a_row_each_with_their_fields() {
        let schema = json!({
            "type": "object",
            "properties": {"input": {"$ref": "#/$defs/InputArg"}},
            "required": ["input"],
            "$defs": {"InputArg": {"oneOf": [
                {"type": "object", "properties": {
                    "type": {"type": "string", "const": "audio"},
                    "channel": {"type": "integer"}}, "required": ["type", "channel"]},
                {"type": "object", "description": "Nothing feeds it.", "properties": {
                    "type": {"type": "string", "const": "none"}}, "required": ["type"]}
            ]}}
        });
        let rows = rows(&schema);
        let text: Vec<(String, usize, String, bool)> = rows
            .iter()
            .map(|r| (r.path.clone(), r.depth, r.notes.clone(), r.form))
            .collect();
        assert_eq!(rows[0].ty, "one of 2 forms");
        assert_eq!(
            text[1..],
            [
                ("input".into(), 1, "Form 1: type = \"audio\"".into(), true),
                ("input.type".into(), 2, "Always \"audio\".".into(), false),
                ("input.channel".into(), 2, String::new(), false),
                (
                    "input".into(),
                    1,
                    "Form 2: type = \"none\". Nothing feeds it.".into(),
                    true
                ),
                ("input.type".into(), 2, "Always \"none\".".into(), false),
            ]
        );
    }

    #[test]
    fn a_result_that_is_not_an_object_is_one_row() {
        let rows = rows(&json!({"type": ["integer", "null"], "format": "uint64", "minimum": 0}));
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].path.as_str(), rows[0].ty.as_str()),
            ("", "integer or null")
        );
        assert_eq!(summary(&json!({"type": "null"})), "null");
        assert_eq!(
            summary(&json!({"$schema": "x", "title": "AnyValue"})),
            "not described"
        );
        let list = super::rows(&json!({"type": "array", "items": {"$ref": "#/$defs/P"},
            "$defs": {"P": {"type": "object", "properties": {"id": {"type": "integer"}},
            "required": ["id"]}}}));
        assert_eq!(
            list.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            ["", "[].id"]
        );
    }

    #[test]
    fn a_type_inside_itself_stops() {
        let schema = json!({"type": "object", "properties": {"node": {"$ref": "#/$defs/N"}},
            "$defs": {"N": {"type": "object", "properties": {
                "children": {"type": "array", "items": {"$ref": "#/$defs/N"}}}}}});
        let rows = rows(&schema);
        assert!(rows.len() < 6, "{rows:#?}");
    }
}
