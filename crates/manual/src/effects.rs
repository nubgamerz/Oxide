//! The built-in effects, from `effects.json` (written from daw-core's `native::KINDS` by
//! `effect_docs_are_current`, which fails the quick tier while it is stale; read here so this
//! crate stays off daw-core's build, as [`crate::catalog`] does for the MCP actions).
//!
//! Two extensions use it (27.5):
//!
//! - `::: params <id>` (a block with nothing inside) becomes the effect's parameter table:
//!   each parameter's range or choices, its default, and whether it can have an automation
//!   lane, with the effect's delay above it. The effect is documented under the heading the
//!   block sits under, which `[[effect:<id>]]` links to; a manual that documents any effect
//!   must document every one, once.
//! - `[[effect:<id>]]`: a link to that heading, the effect's name as its text.

use std::fmt::Write as _;

use serde::Deserialize;

use crate::html::escape;

const EFFECTS: &str = include_str!("effects.json");

/// The placeholder a `::: params` block leaves in the Markdown: `<!--oxide-params-<id>` ...
/// `-->`.
pub const PLACEHOLDER: &str = "<!--oxide-params-";

#[derive(Debug, Clone, Deserialize)]
pub struct Param {
    pub name: String,
    pub min: String,
    pub max: String,
    pub default: String,
    /// Every value it takes, for a switch or a short list of named steps.
    pub choices: Option<Vec<String>>,
    pub automatable: bool,
    /// The switch that moves it with others (Amplify's Link Sliders).
    pub link: Option<String>,
    /// Set by the effect itself, shown, not set (the Space Reverb's room dimensions).
    pub output: bool,
    /// Reshapes the file the effect plays (the reverbs' impulses).
    pub shapes_file: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Effect {
    pub id: String,
    pub name: String,
    /// The plug-in browser's group.
    pub category: String,
    /// Samples of delay at its defaults, at [`Effects::rate`].
    pub latency: u64,
    pub params: Vec<Param>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Effects {
    pub rate: f64,
    pub effects: Vec<Effect>,
}

/// Every built-in effect, in the browser's order.
pub fn all() -> Effects {
    serde_json::from_str(EFFECTS).expect("effects.json is daw-core's own output")
}

/// An effect's parameter table, and its plain text for the search.
pub fn table(effect: &Effect, rate: f64) -> (String, String) {
    let mut html = String::new();
    let mut plain = String::new();
    let delay = if effect.latency == 0 {
        "No delay.".to_string()
    } else {
        format!(
            "Delay: {} samples at {} kHz ({:.1} ms), compensated.",
            effect.latency,
            rate / 1000.0,
            effect.latency as f64 * 1000.0 / rate
        )
    };
    let _ = write!(
        html,
        "<p class=\"ox-safety\">{} parameters. {}</p>\n<table class=\"ox-params ox-effect\">\
         <thead><tr><th>Parameter</th><th>Range</th><th>Default</th><th>Notes</th></tr>\
         </thead><tbody>\n",
        effect.params.len(),
        escape(&delay)
    );
    for p in &effect.params {
        let range = match &p.choices {
            Some(choices) => choices.join(", "),
            None => format!("{} to {}", p.min, p.max),
        };
        let mut notes = Vec::new();
        if p.output {
            notes.push("Set by the effect: shown, not set".to_string());
        } else if !p.automatable {
            notes.push("No automation lane".to_string());
        }
        if p.shapes_file {
            notes.push("Remakes the impulse".to_string());
        }
        if let Some(link) = &p.link {
            if link != &p.name {
                notes.push(format!("Moves with the others while {link} is on"));
            }
        }
        let _ = writeln!(
            html,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape(&p.name),
            escape(&range),
            escape(&p.default),
            escape(&notes.join(". "))
        );
        plain.push_str(&p.name);
        plain.push(' ');
    }
    html.push_str("</tbody></table>\n");
    (html, plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_read_with_its_parameters() {
        let all = all();
        assert!(all.effects.len() >= 27, "{}", all.effects.len());
        let amplify = all.effects.iter().find(|e| e.id == "amplify").unwrap();
        assert_eq!(amplify.name, "Amplify");
        let left = &amplify.params[0];
        assert_eq!(
            (left.min.as_str(), left.max.as_str(), left.default.as_str()),
            ("-96.00 dB", "+48.00 dB", "0.00 dB")
        );
        assert_eq!(left.link.as_deref(), Some("Link Sliders"));
        let link = &amplify.params[2];
        assert_eq!(
            link.choices.as_deref(),
            Some(&["Off".into(), "On".into()][..])
        );
        assert!(!link.automatable);
    }

    #[test]
    fn a_table_lists_ranges_choices_and_lanes() {
        let all = all();
        let amplify = all.effects.iter().find(|e| e.id == "amplify").unwrap();
        let (html, plain) = table(amplify, all.rate);
        assert!(html.contains("<td>Left Gain</td><td>-96.00 dB to +48.00 dB</td><td>0.00 dB</td>"));
        assert!(html.contains("Moves with the others while Link Sliders is on"));
        assert!(html.contains(
            "<td>Link Sliders</td><td>Off, On</td><td>On</td><td>No automation lane</td>"
        ));
        assert!(html.contains("No delay."));
        assert!(plain.contains("Right Gain"));
    }
}
