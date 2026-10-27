//! The browser's search (`assets/manual.js`) ranks as the generator's (`search::query`) does:
//! both run on the real manual's index and must return the same entries in the same order with
//! the same scores. Needs Node.js; without it the test says so and passes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use manual::search;

const QUESTIONS: &[&str] = &[
    "dsp",
    "processing block",
    "choppy audio",
    "How do I stop the audio crackling?",
    "cpu",
    "peak",
    "input gaps",
    "get_stats",
    "buffer size 64",
    "proc",
    "the",
    "zzzz nothing",
];

#[test]
fn the_browser_ranks_as_the_generator_does() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("SKIP: Node.js isn't on the PATH, so manual.js isn't run");
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual");
    let source = manual::Source::read(&dir, &dir.join("shots")).expect("docs/manual reads");
    let built = manual::build(
        &source,
        &manual::Options::default(),
        &manual::mcp::Reference::load(),
    );

    let work = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("manual-search-js");
    fs::create_dir_all(&work).unwrap();
    fs::write(
        work.join("search-index.js"),
        &built.files["search-index.js"],
    )
    .unwrap();
    fs::write(work.join("manual.js"), &built.files["assets/manual.js"]).unwrap();
    fs::write(
        work.join("questions.json"),
        serde_json::to_string(QUESTIONS).unwrap(),
    )
    .unwrap();
    fs::write(
        work.join("run.js"),
        "globalThis.window = globalThis;\n\
         require('./search-index.js');\n\
         require('./manual.js');\n\
         const questions = require('./questions.json');\n\
         const out = questions.map(q => ({\n\
           hits: OxideManual.search(window.OXIDE_SEARCH, q).map(h => [h.entry, h.score]),\n\
           snippet: (() => { const h = OxideManual.search(window.OXIDE_SEARCH, q)[0];\n\
             return h ? OxideManual.snippet(window.OXIDE_SEARCH.entries[h.entry].text,\n\
               OxideManual.question(q)) : ''; })(),\n\
         }));\n\
         process.stdout.write(JSON.stringify(out));\n",
    )
    .unwrap();
    let output = Command::new("node")
        .arg("run.js")
        .current_dir(&work)
        .output()
        .expect("node runs");
    assert!(
        output.status.success(),
        "node: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let results: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).expect("node prints JSON");

    for (question, js) in QUESTIONS.iter().zip(&results) {
        let rust: Vec<(usize, u32)> = search::query(&built.index, question)
            .iter()
            .map(|h| (h.entry, h.score))
            .collect();
        let js: Vec<(usize, u32)> = js["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| {
                (
                    h[0].as_u64().unwrap() as usize,
                    h[1].as_u64().unwrap() as u32,
                )
            })
            .collect();
        assert_eq!(js, rust, "{question:?}");
    }

    // The manual's own words find its pages.
    let top = |q: &str| {
        let hit = search::query(&built.index, q)[0];
        let entry = &built.index.entries[hit.entry];
        format!("{}#{}", built.index.pages[entry.page].file, entry.anchor)
    };
    assert!(top("choppy audio").starts_with("choppy-audio.html#"));
    // The setting is explained on the audio device's page; raising it is the crackles' fix.
    assert!(top("processing block").starts_with("audio-device.html#"));
    assert!(top("raise the processing block").starts_with("choppy-audio.html#"));
    assert_eq!(top("get_stats"), "mcp-devices.html#get_stats");
    // A snippet marks the words it matched.
    let snippet = results[1]["snippet"].as_str().unwrap();
    assert!(
        snippet.to_lowercase().contains("<mark>processing</mark>"),
        "{snippet}"
    );
}
