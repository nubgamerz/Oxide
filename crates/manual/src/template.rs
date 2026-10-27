//! The page around a page's body: the header with the search, the chapters on the left, the
//! page's own contents on the right, previous and next, and the version in the footer.

use std::fmt::Write as _;

use crate::html::escape;
use crate::render::Heading;

/// A chapter in the sidebar: its title and its pages (id, title).
pub struct Chapter {
    pub title: String,
    pub pages: Vec<(String, String)>,
}

pub struct Page<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub headings: &'a [Heading],
    pub chapters: &'a [Chapter],
    pub prev: Option<(&'a str, &'a str)>,
    pub next: Option<(&'a str, &'a str)>,
    pub version: &'a str,
}

pub fn file_of(id: &str) -> String {
    format!("{id}.html")
}

pub fn page(p: &Page) -> String {
    let mut out = String::with_capacity(p.body.len() + 8192);
    let _ = write!(
        out,
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"color-scheme\" content=\"light dark\">\n\
         <meta name=\"generator\" content=\"Oxide manual {version}\">\n\
         <title>{title} \u{b7} Oxide manual</title>\n\
         <link rel=\"stylesheet\" href=\"assets/manual.css\">\n\
         <script src=\"search-index.js\" defer></script>\n\
         <script src=\"assets/manual.js\" defer></script>\n\
         </head>\n<body>\n\
         <input type=\"checkbox\" id=\"ox-nav-toggle\" class=\"ox-nav-toggle\" hidden>\n\
         <header class=\"ox-header\">\n\
         <label for=\"ox-nav-toggle\" class=\"ox-menu\" title=\"Chapters\" aria-label=\"Show the chapters\">\
         <span></span><span></span><span></span></label>\n\
         <a class=\"ox-brand\" href=\"index.html\">Oxide <span>manual</span></a>\n\
         <div class=\"ox-search\" role=\"search\">\n\
         <input id=\"ox-search\" type=\"search\" placeholder=\"Search the manual\" \
         autocomplete=\"off\" spellcheck=\"false\" aria-label=\"Search the manual\" \
         role=\"combobox\" aria-autocomplete=\"list\" aria-controls=\"ox-results\" \
         aria-expanded=\"false\"><kbd class=\"ox-slash\" title=\"Press / to search\">/</kbd>\n\
         <ul id=\"ox-results\" class=\"ox-results\" role=\"listbox\" hidden></ul>\n\
         </div>\n</header>\n<div class=\"ox-layout\">\n",
        version = escape(p.version),
        title = escape(p.title),
    );

    out.push_str("<nav class=\"ox-sidebar\" aria-label=\"Chapters\">\n");
    for chapter in p.chapters {
        let _ = write!(
            out,
            "<div class=\"ox-chapter\"><h2>{}</h2><ul>",
            escape(&chapter.title)
        );
        for (id, title) in &chapter.pages {
            let current = if id == p.id {
                " aria-current=\"page\""
            } else {
                ""
            };
            let _ = write!(
                out,
                "<li><a href=\"{}\"{current}>{}</a></li>",
                escape(&file_of(id)),
                escape(title)
            );
        }
        out.push_str("</ul></div>\n");
    }
    out.push_str("</nav>\n");

    let _ = write!(
        out,
        "<main class=\"ox-main\">\n<article class=\"ox-page\" id=\"ox-article\">\n{}</article>\n",
        p.body
    );
    out.push_str("<nav class=\"ox-pager\" aria-label=\"Previous and next\">");
    match p.prev {
        Some((id, title)) => {
            let _ = write!(
                out,
                "<a class=\"ox-prev\" rel=\"prev\" href=\"{}\"><span>Previous</span>{}</a>",
                escape(&file_of(id)),
                escape(title)
            );
        }
        None => out.push_str("<span></span>"),
    }
    if let Some((id, title)) = p.next {
        let _ = write!(
            out,
            "<a class=\"ox-next\" rel=\"next\" href=\"{}\"><span>Next</span>{}</a>",
            escape(&file_of(id)),
            escape(title)
        );
    }
    out.push_str("</nav>\n");
    let _ = write!(
        out,
        "<footer class=\"ox-footer\">Oxide {v} \u{b7} This manual documents Oxide {v}.</footer>\n</main>\n",
        v = escape(p.version)
    );

    let contents: Vec<&Heading> = p.headings.iter().filter(|h| h.level <= 3).collect();
    out.push_str("<nav class=\"ox-toc\" aria-label=\"On this page\">");
    if !contents.is_empty() {
        out.push_str("<h2>On this page</h2><ul>");
        for h in contents {
            let _ = write!(
                out,
                "<li class=\"ox-toc-{}\"><a href=\"#{}\">{}</a></li>",
                h.level,
                escape(&h.anchor),
                escape(&h.text)
            );
        }
        out.push_str("</ul>");
    }
    out.push_str("</nav>\n</div>\n</body>\n</html>\n");
    out
}
