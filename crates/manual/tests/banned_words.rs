//! Oxide's built-in effects and its documents stand on their own (dev plan 28.5): no file in
//! the repository names the program its effects were once compared with, or its maker. Every
//! file `git ls-files` lists (tracked, and new files git doesn't ignore) is read; files that
//! aren't UTF-8 text (or hold a NUL byte) are skipped, and a path is checked as well as its
//! text, case-insensitively.
//!
//! A file allowed to keep a mention goes in `tests/banned_words_allowed.txt`: one path a line
//! (`*` within a folder, `**` across folders), then `#` and why. The list is meant to be empty.
//!
//! The words are spelt in parts here so this file passes its own check.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The words, lower case.
fn words() -> [String; 2] {
    [["aud", "ition"].concat(), ["ado", "be"].concat()]
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The allowed path patterns, from `banned_words_allowed.txt`.
fn allowed() -> Vec<String> {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/banned_words_allowed.txt");
    let text = std::fs::read_to_string(&file).expect("tests/banned_words_allowed.txt reads");
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether `path` (with `/`) matches `pattern`: `**` any number of folders, `*` any run of
/// characters within one name.
fn matches(pattern: &str, path: &str) -> bool {
    fn names(p: &[&str], q: &[&str]) -> bool {
        match (p.first(), q.first()) {
            (None, None) => true,
            (Some(&"**"), _) => names(&p[1..], q) || (!q.is_empty() && names(p, &q[1..])),
            (Some(a), Some(b)) => name(a, b) && names(&p[1..], &q[1..]),
            _ => false,
        }
    }
    fn name(p: &str, s: &str) -> bool {
        match p.split_once('*') {
            None => p == s,
            Some((head, rest)) => {
                s.starts_with(head)
                    && (head.len()..=s.len()).any(|i| s.is_char_boundary(i) && name(rest, &s[i..]))
            }
        }
    }
    let p: Vec<&str> = pattern.split('/').collect();
    let q: Vec<&str> = path.split('/').collect();
    names(&p, &q)
}

/// The tracked files, as `git ls-files` gives them.
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
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// Each line of `text` (from 1) holding one of the words, with the line.
fn mentions(text: &str) -> Vec<(usize, String)> {
    let words = words();
    text.lines()
        .enumerate()
        .filter(|(_, l)| {
            let l = l.to_lowercase();
            words.iter().any(|w| l.contains(w.as_str()))
        })
        .map(|(i, l)| (i + 1, l.trim().chars().take(120).collect()))
        .collect()
}

#[test]
fn the_pattern_matcher_works() {
    assert!(matches("docs/manual/**/*.md", "docs/manual/index.md"));
    assert!(matches(
        "docs/manual/**/*.md",
        "docs/manual/effects/effects.md"
    ));
    assert!(!matches("docs/manual/**/*.md", "docs/manual/shots.toml"));
    assert!(matches("docs/releases/*.md", "docs/releases/0.3.1.md"));
    assert!(!matches("docs/releases/*.md", "docs/releases/old/0.1.md"));
    assert!(matches("README.md", "README.md"));
    assert!(!matches("README.md", "docs/README.md"));
}

#[test]
fn the_words_are_found_in_any_case() {
    let w = words();
    let text = format!("one\nAn {} here\nnone\n{} too", w[0].to_uppercase(), w[1]);
    assert_eq!(
        mentions(&text).iter().map(|m| m.0).collect::<Vec<_>>(),
        [2, 4]
    );
}

#[test]
fn no_tracked_file_names_the_reference_program() {
    let allowed = allowed();
    let words = words();
    let mut problems = Vec::new();
    for path in tracked() {
        let lower = path.to_lowercase();
        if words.iter().any(|w| lower.contains(w.as_str())) {
            problems.push(format!("{path}: its path holds a banned word"));
        }
        let Ok(bytes) = std::fs::read(repo().join(&path)) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        if allowed.iter().any(|p| matches(p, &path)) {
            continue;
        }
        problems.extend(
            mentions(&text)
                .iter()
                .map(|(line, text)| format!("{path}:{line}: {text}")),
        );
    }
    assert!(
        problems.is_empty(),
        "{} problem(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}
