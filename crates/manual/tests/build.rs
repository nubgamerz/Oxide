//! The real manual in `docs/manual` builds without errors (as `docs_are_current` keeps the MCP
//! docs current), and a built manual is written to disk with its screenshots.

use std::fs;
use std::path::{Path, PathBuf};

use manual::check::Severity;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn the_manual_in_docs_has_no_broken_references() {
    let dir = repo().join("docs/manual");
    let source = manual::Source::read(&dir, &dir.join("shots")).expect("docs/manual reads");
    let built = manual::build(
        &source,
        &manual::Options::default(),
        &manual::mcp::Reference::load(),
    );
    let errors: Vec<String> = built
        .problems
        .iter()
        .filter(|p| p.severity == Severity::Error)
        .map(|p| p.to_string())
        .collect();
    assert!(errors.is_empty(), "docs/manual: {errors:#?}");
    assert_eq!(built.pages.first().map(String::as_str), Some("index"));
    assert!(built.files.contains_key("index.html"));
    assert!(built.files.contains_key("mcp-reference.html"));
}

#[test]
fn a_built_manual_is_written_with_its_screenshots() {
    let dir = fixtures().join("manual");
    let source = manual::Source::read(&dir, &fixtures().join("shots")).expect("fixture reads");
    let built = manual::build(
        &source,
        &manual::Options {
            strict_shots: true,
            ..manual::Options::default()
        },
        &manual::mcp::Reference::load(),
    );
    assert!(built.problems.is_empty(), "{:#?}", built.problems);
    let html = &built.files["index.html"];
    assert!(html.contains("<img src=\"shots/sample.png\""));
    assert!(html.contains("data-mark=\"transport.dsp\""));
    // A highlight without a label is an outline only.
    assert!(html.contains("data-mark=\"status\" data-rect=\"0 80 160 10\" style=\"left: 0%; top: 88.889%; width: 100%; height: 11.111%\"></span>"));

    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("manual-fixture");
    let _ = fs::remove_dir_all(&out);
    let written = manual::write(&built, &out).expect("written");
    for file in [
        "index.html",
        "mcp-reference.html",
        "search-index.js",
        "search-index.json",
        "assets/manual.css",
        "assets/manual.js",
    ] {
        assert!(out.join(file).is_file(), "{file}");
    }
    // The app's copy of the index (27.3) is the pages' own.
    let json = fs::read_to_string(out.join("search-index.json")).unwrap();
    assert_eq!(
        manual::search::Index::from_json(&json).unwrap(),
        built.index
    );
    // The screenshot is written again (its four colours in a palette), every pixel kept.
    let decode = |path: PathBuf| image::open(path).expect("a PNG").to_rgb8();
    assert_eq!(
        decode(out.join("shots/sample.png")),
        decode(fixtures().join("shots/sample.png"))
    );
    assert_eq!(written.images.len(), 1);
    assert_eq!(written.images[0].2, manual::shots::Encoding::Exact);
    assert_eq!(
        written.images[0].1,
        fs::metadata(out.join("shots/sample.png")).unwrap().len()
    );
    assert!(written.total > written.image_bytes());
    // A second build replaces the first: a stale page goes.
    fs::write(out.join("stale.html"), "old").unwrap();
    manual::write(&built, &out).expect("rewritten");
    assert!(!out.join("stale.html").exists());

    // A folder that isn't a build is never emptied.
    let foreign = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("manual-foreign");
    let _ = fs::remove_dir_all(&foreign);
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("keep.txt"), "mine").unwrap();
    let refused = manual::write(&built, &foreign).unwrap_err().to_string();
    assert!(refused.contains("not touching it"), "{refused}");
    assert!(foreign.join("keep.txt").exists());
}
