//! Small HTML helpers.

/// Escapes text for HTML content and attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A heading's anchor from its text: lower case, letters and digits kept, spaces and dashes
/// become one dash, everything else dropped ("Raise the processing block (64 or 128)" →
/// `raise-the-processing-block-64-or-128`).
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.extend(c.to_lowercase());
        } else if c.is_whitespace() || c == '-' || c == '_' {
            dash = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_plain() {
        assert_eq!(
            slug("Raise the processing block (64 or 128)"),
            "raise-the-processing-block-64-or-128"
        );
        assert_eq!(slug("  DSP %, peak & input gaps "), "dsp-peak-input-gaps");
        assert_eq!(slug("Plug-ins: too heavy?"), "plug-ins-too-heavy");
        assert_eq!(slug("Ünïcode Wörds"), "ünïcode-wörds");
        assert_eq!(slug("!!!"), "");
    }

    #[test]
    fn escaping_covers_attributes() {
        assert_eq!(
            escape(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }
}
