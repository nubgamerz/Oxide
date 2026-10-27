//! Oxide's manual: its search index and ranking (dev plan 27.1, 27.3).
//!
//! The generator (`crates/manual`) builds the index with the pages and writes it twice: as
//! `search-index.js`, loaded by the pages (a classic script: `fetch` is blocked on `file://`),
//! and as `search-index.json`, read by the app (`search_manual`, `open_manual`'s topic). This
//! crate is only serde, so the app links it without the generator's Markdown and image crates.
//!
//! An entry is a part of a page: its top (before the first heading, `anchor` empty) or a
//! section under an h2 or h3. Each keeps its words (lower case, letters and digits) by field,
//! and its text for the snippet. [`query`] is the ranking, which `assets/manual.js`'s
//! `OxideManual.search` implements again for the browser (`manual`'s `tests/search_js.rs` runs
//! both on the same index and compares); the app's `search_manual` action uses this one.
//!
//! Ranking: an entry scores for each word of the question it matches (a word matching it
//! exactly, or starting with it), in its best field: the page's title (30 on the page's top
//! entry, 3 on its sections), the section's heading (10), the text (1); an exact word counts
//! twice a prefix. Entries matching more of the question's words always come first, then the
//! higher score, then the manual's order. Common words ("how", "the", "do") are left out of
//! the question unless it is only those.

use serde::{Deserialize, Serialize};

/// One page in the index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexPage {
    /// The page's file (`choppy-audio.html`).
    pub file: String,
    pub title: String,
    /// The title's words.
    pub words: Vec<String>,
}

/// One searchable part of a page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexEntry {
    /// Index into `pages`.
    pub page: usize,
    /// The heading's anchor; empty for the page's top.
    pub anchor: String,
    /// The heading's text; empty for the page's top.
    pub heading: String,
    /// The section's text, for the snippet.
    pub text: String,
    /// The heading's words.
    pub hw: Vec<String>,
    /// The text's words, each once.
    pub tw: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Index {
    pub version: String,
    pub pages: Vec<IndexPage>,
    pub entries: Vec<IndexEntry>,
}

/// Words the question may drop: they match almost everything.
pub const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "how",
    "i", "if", "in", "is", "it", "my", "of", "on", "or", "the", "to", "what", "when", "where",
    "which", "why", "with", "you",
];

pub const TITLE: u32 = 30;
pub const TITLE_IN_SECTION: u32 = 3;
pub const HEADING: u32 = 10;
pub const TEXT: u32 = 1;
/// A question word's score is its field's weight times this for an exact word, once for a
/// prefix; an entry matching more words always ranks above one matching fewer.
pub const EXACT: u32 = 2;
pub const PER_WORD: u32 = 1000;
/// A section whose heading holds every word of a question of two or more (27.5; one word that
/// is a page's title still opens the page): it is the answer, so it
/// ranks above a page whose title holds one of the words and its text the others ("connect
/// Claude" opens Settings' Connect Claude, not "Claude can't reach Oxide"). Less than
/// [`PER_WORD`], so an entry matching more words still comes first.
pub const WHOLE_HEADING: u32 = 100;

/// Lower-case words of letters and digits; anything else separates them.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn unique(words: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    words
        .into_iter()
        .filter(|w| seen.insert(w.clone()))
        .collect()
}

impl Index {
    pub fn add_page(&mut self, file: &str, title: &str) -> usize {
        self.pages.push(IndexPage {
            file: file.to_string(),
            title: title.to_string(),
            words: unique(words(title)),
        });
        self.pages.len() - 1
    }

    pub fn add_entry(&mut self, page: usize, anchor: &str, heading: &str, text: &str) {
        self.entries.push(IndexEntry {
            page,
            anchor: anchor.to_string(),
            heading: heading.to_string(),
            text: text.to_string(),
            hw: unique(words(heading)),
            tw: unique(words(text)),
        });
    }

    /// `search-index.js`: the index as a global for the pages' classic scripts.
    pub fn script(&self) -> String {
        let json = serde_json::to_string(self).expect("the index serializes");
        // `</` can't end the script early even if a page's text holds "</script>".
        format!(
            "// Oxide's manual: the search index (generated; do not edit).\nwindow.OXIDE_SEARCH = {};\n",
            json.replace("</", "<\\/")
        )
    }
}

/// The file the generator writes the index to for the app, beside `search-index.js`.
pub const JSON_FILE: &str = "search-index.json";

impl Index {
    /// `search-index.json`: the object `search-index.js` sets, for the app to read.
    pub fn json(&self) -> String {
        serde_json::to_string(self).expect("the index serializes")
    }

    /// Reads [`Index::json`]'s text back.
    pub fn from_json(text: &str) -> Result<Index, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// A page's id: its file without `.html` (`choppy-audio`), as links and F1's table name it.
    pub fn page_id(&self, page: usize) -> &str {
        let file = &self.pages[page].file;
        file.strip_suffix(".html").unwrap_or(file)
    }

    /// The page called `id`, if the manual has it.
    pub fn page(&self, id: &str) -> Option<usize> {
        (0..self.pages.len()).find(|&n| self.page_id(n) == id)
    }

    /// The entry for `anchor` on page `page` (`""`: the page's top), if it has that section.
    /// Only `##` and `###` headings are entries.
    pub fn entry(&self, page: usize, anchor: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.page == page && e.anchor == anchor)
    }
}

/// A piece of `text` of about `length` characters around the first word that starts with a
/// word of `asked` (a question, as [`query`] takes it), as plain text with `…` where it was
/// cut: what the browser's results show (`manual.js`'s `snippet`) without its HTML marks.
pub fn snippet(text: &str, asked: &str, length: usize) -> String {
    let question = question(asked);
    let chars: Vec<char> = text.chars().collect();
    // Where the first matching word starts (0 when none does).
    let mut at = 0;
    let mut n = 0;
    while n < chars.len() {
        if !chars[n].is_alphanumeric() {
            n += 1;
            continue;
        }
        let start = n;
        while n < chars.len() && chars[n].is_alphanumeric() {
            n += 1;
        }
        let word = chars[start..n].iter().collect::<String>().to_lowercase();
        if question.iter().any(|q| word.starts_with(q.as_str())) {
            at = start;
            break;
        }
    }
    let mut start = at.saturating_sub(50);
    if start > 0 {
        if let Some(space) = (start..at).find(|&i| chars[i] == ' ') {
            start = space + 1;
        }
    }
    let mut end = (start + length).min(chars.len());
    if end < chars.len() {
        if let Some(cut) = (at + 1..end).rev().find(|&i| chars[i] == ' ') {
            end = cut;
        }
    }
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        chars[start..end].iter().collect::<String>().trim(),
        if end < chars.len() { "…" } else { "" }
    )
}

/// The question's words, without the common ones (unless that leaves none).
pub fn question(text: &str) -> Vec<String> {
    let all = unique(words(text));
    let kept: Vec<String> = all
        .iter()
        .filter(|w| !STOP_WORDS.contains(&w.as_str()))
        .cloned()
        .collect();
    if kept.is_empty() {
        all
    } else {
        kept
    }
}

/// How well one field's words match one question word: `EXACT` for the word itself, 1 for a
/// word starting with it, 0 for neither.
fn field(words: &[String], q: &str) -> u32 {
    let mut best = 0;
    for w in words {
        if w == q {
            return EXACT;
        }
        if w.starts_with(q) {
            best = 1;
        }
    }
    best
}

/// One result: an entry and its score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub entry: usize,
    pub score: u32,
}

/// Every entry matching any word of `text`, best first.
pub fn query(index: &Index, text: &str) -> Vec<Hit> {
    let question = question(text);
    if question.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (n, entry) in index.entries.iter().enumerate() {
        let title_weight = if entry.anchor.is_empty() {
            TITLE
        } else {
            TITLE_IN_SECTION
        };
        let title = &index.pages[entry.page].words;
        let mut matched = 0;
        let mut sum = 0;
        let mut in_heading = 0;
        for q in &question {
            let heading = field(&entry.hw, q);
            if heading > 0 {
                in_heading += 1;
            }
            let best = (field(title, q) * title_weight)
                .max(heading * HEADING)
                .max(field(&entry.tw, q) * TEXT);
            if best > 0 {
                matched += 1;
                sum += best;
            }
        }
        if !entry.anchor.is_empty() && question.len() > 1 && in_heading == question.len() {
            sum += WHOLE_HEADING;
        }
        if matched > 0 {
            hits.push(Hit {
                entry: n,
                score: matched * PER_WORD + sum,
            });
        }
    }
    // Stable: equal scores keep the manual's order.
    hits.sort_by_key(|h| std::cmp::Reverse(h.score));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Index {
        let mut index = Index::default();
        let a = index.add_page("monitoring.html", "Monitoring while recording");
        index.add_entry(a, "", "", "Hear yourself through your plug-ins.");
        index.add_entry(a, "latency", "Latency", "The buffer adds delay.");
        let b = index.add_page("recording.html", "Recording");
        index.add_entry(b, "", "", "Arm a track and press Record.");
        index.add_entry(
            b,
            "input-monitoring",
            "Input monitoring",
            "Turn it on per track.",
        );
        let c = index.add_page("mixer.html", "The mixer");
        index.add_entry(c, "", "", "Faders, and the monitoring of sends.");
        index
    }

    fn files(index: &Index, hits: &[Hit]) -> Vec<String> {
        hits.iter()
            .map(|h| {
                let e = &index.entries[h.entry];
                format!("{}#{}", index.pages[e.page].file, e.anchor)
            })
            .collect()
    }

    #[test]
    fn a_heading_holding_every_word_ranks_above_a_title_holding_one() {
        let mut index = Index::default();
        let a = index.add_page("trouble.html", "Claude can't reach Oxide");
        index.add_entry(a, "", "", "When Claude can't connect, check these.");
        let b = index.add_page("settings.html", "Settings");
        index.add_entry(
            b,
            "connect-claude",
            "Connect Claude",
            "Configure each client.",
        );
        let first = |q: &str| {
            let hit = query(&index, q)[0];
            index.entries[hit.entry].anchor.clone()
        };
        assert_eq!(first("connect Claude"), "connect-claude");
        // A word the heading lacks ("can't" is "can", a stop word, and "t"): the page wins.
        assert_eq!(first("Claude can't connect"), "");
    }

    #[test]
    fn titles_rank_above_headings_above_text() {
        let index = sample();
        let hits = query(&index, "monitoring");
        assert_eq!(
            files(&index, &hits),
            [
                "monitoring.html#",
                "recording.html#input-monitoring",
                // A section of the titled page: the title counts a little there.
                "monitoring.html#latency",
                "mixer.html#",
            ]
        );
        let scores: Vec<u32> = hits.iter().map(|h| h.score).collect();
        assert_eq!(scores, [1060, 1020, 1006, 1002]);
    }

    #[test]
    fn prefixes_match_and_exact_words_beat_them() {
        let index = sample();
        let prefix = query(&index, "monitor");
        assert_eq!(files(&index, &prefix)[0], "monitoring.html#");
        assert_eq!(prefix.len(), 4);
        let exact = query(&index, "monitoring");
        assert!(exact[0].score > prefix[0].score);
    }

    #[test]
    fn more_words_matched_win_and_common_words_are_dropped() {
        let index = sample();
        assert_eq!(
            question("How do I turn on the monitoring?"),
            ["turn", "monitoring"]
        );
        assert_eq!(question("how to"), ["how", "to"]);
        let hits = query(&index, "how do I turn on monitoring");
        assert_eq!(files(&index, &hits)[0], "recording.html#input-monitoring");
        assert!(query(&index, "").is_empty());
        assert!(query(&index, "zzz").is_empty());
    }

    #[test]
    fn the_json_reads_back_and_finds_pages_and_sections() {
        let index = sample();
        let back = Index::from_json(&index.json()).unwrap();
        assert_eq!(back, index);
        assert_eq!(back.page_id(1), "recording");
        assert_eq!(back.page("mixer"), Some(2));
        assert_eq!(back.page("nope"), None);
        assert_eq!(back.entry(1, "input-monitoring"), Some(3));
        assert_eq!(back.entry(1, ""), Some(2));
        assert_eq!(back.entry(1, "latency"), None);
    }

    #[test]
    fn a_snippet_is_cut_round_the_first_match_at_spaces() {
        let text = "One two three four five six seven eight nine ten eleven twelve thirteen \
                    fourteen fifteen sixteen. The processing block gathers buffers into a \
                    bigger block, which costs latency but saves heavy plug-ins time.";
        let s = snippet(text, "how do I raise the processing", 90);
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert!(s.contains("processing block"), "{s}");
        assert!(s.chars().count() <= 92, "{s}");
        // No match: from the start.
        assert!(snippet(text, "zzz", 20).starts_with("One two"));
        // Short text: whole.
        assert_eq!(snippet("Arm a track.", "arm", 170), "Arm a track.");
    }

    #[test]
    fn the_script_is_a_classic_global() {
        let mut index = sample();
        index.add_entry(0, "x", "x", "a </script> in text");
        let script = index.script();
        assert!(script.starts_with("// Oxide's manual"));
        assert!(script.contains("window.OXIDE_SEARCH = {"));
        assert!(!script.contains("</script>"));
    }
}
