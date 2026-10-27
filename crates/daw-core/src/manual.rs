//! The manual from inside the app (dev plan 27.3): its search index, read from the built
//! manual's folder (`manual\` beside the program, [`crate::session::Session::manual_dir`]), so
//! `search_manual` can answer "how do I…" from the pages, and `open_manual` can turn a topic
//! or a page id into the page to show.
//!
//! The index is `search-index.json`, written by the generator (`crates/manual`) beside the
//! pages' `search-index.js`; the ranking is the pages' own (`manual_search::query`). It is read
//! afresh on each call: a few hundred kilobytes parsed in a few milliseconds, and a manual
//! rebuilt while the app runs is the one searched.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
pub use manual_search::Index;
use serde::Serialize;

/// The manual's first page.
pub const INDEX_PAGE: &str = "index";

/// The pages' file name for `page` (`choppy-audio` → `choppy-audio.html`).
pub fn file_name(page: &str) -> String {
    format!("{page}.html")
}

/// Reads the index of the manual in `dir`.
pub fn load(dir: &Path) -> Result<Index> {
    let path = dir.join(manual_search::JSON_FILE);
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "the manual's search index ({}) can't be read",
            path.display()
        )
    })?;
    Index::from_json(&text).with_context(|| format!("{} isn't a search index", path.display()))
}

/// A place in the manual: a page and, optionally, a section on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Target {
    /// The page's id (its file without `.html`): `choppy-audio`.
    pub page: String,
    /// A heading's anchor on it (`raise-the-processing-block`); `None`: the page's top.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// The page's title, and the section's heading when there is an anchor (empty when the
    /// index wasn't read: F1 with no manual beside the program).
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
}

impl Target {
    /// The page `page` at `anchor`, with no titles (F1's table names pages without reading
    /// the index; [`check`] fills them in).
    pub fn new(page: &str, anchor: Option<&str>) -> Target {
        Target {
            page: page.to_string(),
            anchor: anchor.filter(|a| !a.is_empty()).map(str::to_string),
            title: String::new(),
            heading: None,
        }
    }

    /// The page's file in the manual's folder `dir`.
    pub fn file(&self, dir: &Path) -> PathBuf {
        dir.join(file_name(&self.page))
    }
}

/// `page` (with `#anchor` in it, or `anchor` given apart) checked against the index: the
/// target with its titles, or an error naming what isn't there and what is.
pub fn check(index: &Index, page: &str, anchor: Option<&str>) -> Result<Target> {
    let (page, anchor) = match page.split_once('#') {
        Some((p, a)) => (p, Some(a).or(anchor)),
        None => (page, anchor),
    };
    let page = page.trim().trim_end_matches(".html");
    let page = if page.is_empty() { INDEX_PAGE } else { page };
    let Some(n) = index.page(page) else {
        let ids: Vec<&str> = (0..index.pages.len()).map(|n| index.page_id(n)).collect();
        bail!(
            "the manual has no page '{page}'; its pages are: {}",
            ids.join(", ")
        );
    };
    let mut target = Target::new(page, anchor);
    target.title = index.pages[n].title.clone();
    if let Some(anchor) = &target.anchor {
        let Some(e) = index.entry(n, anchor) else {
            let anchors: Vec<&str> = index
                .entries
                .iter()
                .filter(|e| e.page == n && !e.anchor.is_empty())
                .map(|e| e.anchor.as_str())
                .collect();
            bail!(
                "the page '{page}' has no section '{anchor}'; its sections are: {}",
                if anchors.is_empty() {
                    "none".to_string()
                } else {
                    anchors.join(", ")
                }
            );
        };
        target.heading = Some(index.entries[e].heading.clone());
    }
    Ok(target)
}

/// One answer to a question: a page, or a section of one.
#[derive(Debug, Clone, Serialize)]
pub struct Passage {
    /// The page's id (open_manual's `page`).
    pub page: String,
    pub title: String,
    /// The section's anchor and heading; absent for the page's opening text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    /// How well it matched: the question's words matched times 1000, plus where they were
    /// (the page's title counts most, then the heading, then the text). Higher is better.
    pub score: u32,
    /// A couple of lines around the first word that matched.
    pub snippet: String,
    /// The section's whole text (the page's opening text for a page), as plain text.
    pub text: String,
}

/// The best `limit` passages of the manual for `question`, best first.
pub fn search(index: &Index, question: &str, limit: usize) -> Vec<Passage> {
    manual_search::query(index, question)
        .into_iter()
        .take(limit)
        .map(|hit| {
            let entry = &index.entries[hit.entry];
            let anchor = (!entry.anchor.is_empty()).then(|| entry.anchor.clone());
            Passage {
                page: index.page_id(entry.page).to_string(),
                title: index.pages[entry.page].title.clone(),
                heading: anchor.as_ref().map(|_| entry.heading.clone()),
                anchor,
                score: hit.score,
                snippet: manual_search::snippet(&entry.text, question, 200),
                text: entry.text.clone(),
            }
        })
        .collect()
}

/// The place `topic` (a question, or a few words) is best answered: the first passage
/// [`search`] finds.
pub fn topic(index: &Index, topic: &str) -> Result<Target> {
    let Some(best) = search(index, topic, 1).into_iter().next() else {
        bail!("nothing in the manual matches '{topic}'; try other words, or search_manual");
    };
    Ok(Target {
        page: best.page,
        anchor: best.anchor,
        title: best.title,
        heading: best.heading,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Index {
        let mut index = Index::default();
        let a = index.add_page("index.html", "Oxide manual");
        index.add_entry(a, "", "", "Welcome.");
        let b = index.add_page("choppy-audio.html", "Crackling, choppy audio and high DSP");
        index.add_entry(b, "", "", "When playback crackles.");
        index.add_entry(
            b,
            "raise-the-processing-block",
            "Raise the processing block",
            "Gather buffers into a bigger block.",
        );
        index
    }

    #[test]
    fn pages_and_sections_are_checked_by_id() {
        let index = sample();
        let t = check(&index, "choppy-audio", Some("raise-the-processing-block")).unwrap();
        assert_eq!(t.title, "Crackling, choppy audio and high DSP");
        assert_eq!(t.heading.as_deref(), Some("Raise the processing block"));
        // `page#anchor` in one, `.html` tolerated, empty is the first page.
        assert_eq!(
            check(&index, "choppy-audio.html#raise-the-processing-block", None).unwrap(),
            t
        );
        assert_eq!(check(&index, "", None).unwrap().page, "index");
        let e = check(&index, "mixer", None).unwrap_err().to_string();
        assert!(
            e.contains("no page 'mixer'") && e.contains("choppy-audio"),
            "{e}"
        );
        let e = check(&index, "choppy-audio", Some("nope"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("raise-the-processing-block"), "{e}");
    }

    #[test]
    fn a_topic_is_the_best_passage() {
        let index = sample();
        let t = topic(&index, "how do I gather buffers").unwrap();
        assert_eq!(t.page, "choppy-audio");
        assert_eq!(t.anchor.as_deref(), Some("raise-the-processing-block"));
        assert!(topic(&index, "zzz").is_err());
        let found = search(&index, "crackles", 5);
        assert_eq!(found[0].page, "choppy-audio");
        assert_eq!(found[0].anchor, None);
        assert_eq!(found[0].snippet, "When playback crackles.");
    }

    #[test]
    fn a_target_names_its_file() {
        let t = Target::new("settings", Some(""));
        assert_eq!(t.anchor, None);
        assert_eq!(t.file(Path::new("x")), Path::new("x").join("settings.html"));
    }
}
