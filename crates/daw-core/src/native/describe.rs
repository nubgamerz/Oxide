//! The built-in effects described for the manual (dev plan 27.5): every [`KINDS`] entry's
//! name, browser group, delay and parameters as the editor shows them (ranges, defaults and
//! choices in display text, through [`ParamSpec::display`]), as JSON.
//!
//! `effect_docs_are_current` (in `api/actions/tests.rs`, beside `docs_are_current`) writes it to
//! `crates/manual/src/effects.json` with `UPDATE_DOCS` set and fails the quick tier while it is
//! stale; the manual's generator reads that file, so it stays off daw-core's build, as it does
//! for the MCP catalog. Each `::: params <id>` block in `docs/manual` is a table made from it.

use serde_json::{json, Value};

use super::{Kind, ParamSpec, KINDS};

/// The rate the delays are given at: the one the manual quotes.
pub const RATE: f64 = 48_000.0;

/// A stepped parameter with no more choices than this lists them; more is a range.
const MOST_CHOICES: u32 = 16;

/// Every built-in effect, in the browser's order.
pub fn effects() -> Value {
    json!({
        "rate": RATE,
        "effects": KINDS.iter().map(|k| effect(k)).collect::<Vec<_>>(),
    })
}

fn effect(kind: &Kind) -> Value {
    let defaults: Vec<f64> = kind.params.iter().map(|p| p.default).collect();
    let latency = match kind.latency {
        Some(of) => of(RATE, &defaults),
        None => (kind.make)(RATE).latency(),
    };
    let outputs = kind.reads.map_or(&[][..], |r| r.outputs);
    let shaped_by = kind.reads.map_or(&[][..], |r| r.shaped_by);
    let params: Vec<Value> = kind
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| param(p, outputs.contains(&i), shaped_by.contains(&i)))
        .collect();
    json!({
        "id": kind.id,
        "name": kind.name,
        "category": kind.category,
        "latency": latency,
        "curve": kind.response.is_some(),
        "transfer": kind.transfer.is_some(),
        "learns": kind.capture.is_some(),
        "scans": kind.learns.map(|l| l.scans.to_vec()),
        "file": kind.reads.filter(|r| !r.makes()).map(|r| r.what),
        "makes_impulse": kind.reads.is_some_and(|r| r.makes()),
        "params": params,
    })
}

fn param(p: &ParamSpec, output: bool, shapes_file: bool) -> Value {
    let switch = p.steps == 1 && p.min == 0.0 && p.max == 1.0;
    let choices: Option<Vec<String>> =
        (switch || (p.steps > 0 && p.steps <= MOST_CHOICES && p.format.is_some())).then(|| {
            (0..=p.steps)
                .map(|s| p.display(p.min + f64::from(s) / f64::from(p.steps) * (p.max - p.min)))
                .collect()
        });
    // A switch's choices read On, Off unless it names them (the Phaser's Shape).
    json!({
        "name": p.name,
        "min": p.display(p.min),
        "max": p.display(p.max),
        "default": p.display(p.default),
        "choices": choices,
        "automatable": p.automatable && !output,
        "link": p.link.map(|l| l.split(':').next().unwrap_or(l)),
        "output": output,
        "shapes_file": shapes_file,
    })
}
