//! Oxide is for anyone (dev plan 27.7, 28.2): nothing in the repository names one machine's
//! hardware, plug-ins, drives or files. Two places are searched: the built manual's text (every
//! page, the search indexes, the generated MCP reference and effect tables, and the generated
//! MCP docs beside it), and every file `git ls-files` lists (tracked, and new files git doesn't
//! ignore; `vendor/` is third-party code and is skipped). Files that aren't UTF-8 text (or hold
//! a NUL byte, as pictures do) are skipped: the manual tier's `--neutral` shots keep the
//! screenshots clean (`daw-app/src/neutral.rs`).
//!
//! The names are the maintainer's, so they aren't in the repository either: they are read from
//! `tests/personal_names.local.txt` when it is there (one name a line, `#` starting a comment;
//! `allow: <phrase>` lines keep a phrase that holds one). Without that file there is nothing to
//! look for and the tests pass.
//!
//! A phrase kept on purpose that holds no one's name goes in `tests/personal_names_allowed.txt`
//! with its reason: one phrase a line (the phrase is removed before the search, so it allows
//! only that wording), `#` starting a comment.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// What to look for: names matched case-sensitively as whole words, except those ending in a
/// non-word character (`X:\`), which match anywhere; and phrases taken out first.
struct List {
    names: Vec<String>,
    allowed: Vec<String>,
}

/// One phrase or name a line, `#` starting a comment.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
}

/// The list: the maintainer's names and phrases (`personal_names.local.txt`, if present) and
/// the public allowed phrases (`personal_names_allowed.txt`).
fn list() -> List {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let public = std::fs::read_to_string(tests.join("personal_names_allowed.txt"))
        .expect("tests/personal_names_allowed.txt reads");
    let mut list = List {
        names: Vec::new(),
        allowed: lines(&public).map(str::to_string).collect(),
    };
    if let Ok(local) = std::fs::read_to_string(tests.join("personal_names.local.txt")) {
        for line in lines(&local) {
            match line.strip_prefix("allow:") {
                Some(phrase) => list.allowed.push(phrase.trim().to_string()),
                None => list.names.push(line.to_string()),
            }
        }
    }
    // Longer phrases first, so one that holds a shorter one is taken out whole.
    list.allowed.sort_by_key(|p| std::cmp::Reverse(p.len()));
    list
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Each place `name` appears in `text` as a name (see [`List`]): byte offsets.
fn found(text: &str, name: &str) -> Vec<usize> {
    let whole = name.chars().last().is_some_and(is_word);
    text.match_indices(name)
        .map(|(at, _)| at)
        .filter(|&at| {
            !whole
                || (!text[..at].chars().next_back().is_some_and(is_word)
                    && !text[at + name.len()..].chars().next().is_some_and(is_word))
        })
        .collect()
}

/// Up to 40 bytes either side of `at`, on character boundaries, on one line.
fn around(text: &str, at: usize, len: usize) -> String {
    let mut from = at.saturating_sub(40);
    while !text.is_char_boundary(from) {
        from -= 1;
    }
    let mut to = (at + len + 40).min(text.len());
    while !text.is_char_boundary(to) {
        to += 1;
    }
    text[from..to].replace(['\n', '\r'], " ")
}

/// Every name found in `files` (path, text) once the allowed phrases are taken out.
fn personal(files: &[(String, String)], list: &List) -> Vec<String> {
    let mut hits = Vec::new();
    for (path, text) in files {
        let mut text = text.clone();
        for phrase in &list.allowed {
            text = text.replace(phrase.as_str(), "");
        }
        for name in &list.names {
            for at in found(&text, name) {
                hits.push(format!(
                    "{path}: {name:?} in \"{}\"",
                    around(&text, at, name.len())
                ));
            }
        }
    }
    hits
}

/// The files `git ls-files` lists, tracked and new (not ignored), outside `vendor/`.
fn tracked() -> Vec<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo())
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git ls-files failed");
    String::from_utf8(out.stdout)
        .expect("paths are UTF-8")
        .split('\0')
        .filter(|p| !p.is_empty() && !p.starts_with("vendor/"))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_manual_names_no_ones_own_setup() {
    let list = list();
    if list.names.is_empty() {
        println!("note: no tests/personal_names.local.txt: no names to look for");
        return;
    }
    let dir = repo().join("docs/manual");
    let source = manual::Source::read(&dir, &dir.join("shots")).expect("docs/manual reads");
    let built = manual::build(
        &source,
        &manual::Options::default(),
        &manual::mcp::Reference::load(),
    );
    let mut files: Vec<(String, String)> = built
        .files
        .iter()
        .map(|(path, text)| (format!("out/manual/{path}"), text.clone()))
        .collect();
    // The generated MCP docs beside it: the tool reference and the bridge's catalog.
    for generated in ["docs/mcp-tools.md", "crates/mcp-link/src/catalog.json"] {
        let text = std::fs::read_to_string(repo().join(generated)).expect("generated docs read");
        files.push((generated.to_string(), text));
    }
    assert!(files.iter().any(|(p, _)| p.ends_with("search-index.json")));
    let hits = personal(&files, &list);
    assert!(
        hits.is_empty(),
        "the manual names someone's own setup (generalise it, or allow the phrase with its \
         reason):\n{}",
        hits.join("\n")
    );
}

/// Every file in the repository is for anyone too (28.2): the code, its comments, the scripts,
/// the README and the release notes, and each path.
#[test]
fn no_tracked_file_names_ones_own_setup() {
    let list = list();
    if list.names.is_empty() {
        println!("note: no tests/personal_names.local.txt: no names to look for");
        return;
    }
    let mut files = Vec::new();
    for path in tracked() {
        // The path itself, as a file of its own.
        files.push((format!("{path} (its path)"), path.clone()));
        let Ok(bytes) = std::fs::read(repo().join(&path)) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        files.push((path, text));
    }
    assert!(files.iter().any(|(p, _)| p == "README.md"));
    let hits = personal(&files, &list);
    assert!(
        hits.is_empty(),
        "{} place(s) in the repository name someone's own setup (generalise them, or allow \
         the phrase with its reason):\n{}",
        hits.len(),
        hits.join("\n")
    );
}

#[test]
fn names_are_found_as_whole_words_and_allowed_phrases_are_not() {
    let files = vec![(
        "page.html".to_string(),
        "Load Zither. Zithering on. Kept: Q:\\Songs and \"Q:\\\\x\". Banjo XT.".to_string(),
    )];
    let names = ["Zither", "Banjo", "Q:\\"].map(str::to_string).to_vec();
    let mut list = List {
        names,
        allowed: Vec::new(),
    };
    let hits = personal(&files, &list);
    // "Zither", both drives and "Banjo"; not "Zithering".
    assert_eq!(hits.len(), 4, "{hits:#?}");
    list.allowed = vec!["Banjo XT".to_string(), "Load Zither".to_string()];
    let hits = personal(&files, &list);
    assert_eq!(hits.len(), 2, "{hits:#?}");
    assert!(hits.iter().all(|h| h.contains("\"Q:\\\\\"")), "{hits:#?}");
}

#[test]
fn the_local_list_reads_names_and_allowed_phrases() {
    let text = "# a comment\nZither\nallow: Zither Road # a place\n\nQ:\\\n";
    let names: Vec<&str> = lines(text).filter(|l| !l.starts_with("allow:")).collect();
    assert_eq!(names, ["Zither", "Q:\\"]);
    assert_eq!(lines(text).count(), 3);
}
