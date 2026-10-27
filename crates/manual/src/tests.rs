//! The generator's own tests: each extension's HTML, each kind of broken reference, the table
//! of contents, and the search index. The real `docs/manual` is built in `tests/docs.rs`.

use std::path::PathBuf;

use serde_json::Value;

use super::*;
use crate::check::Severity;
use crate::mcp::{Action, Area, Reference};

const CONTENTS: &str = r#"
[[chapter]]
title = "Welcome"
pages = ["index.md"]

[[chapter]]
title = "Guide"
pages = ["guide/other.md", "@mcp-reference", "@mcp-actions"]
"#;

const OTHER: &str = "# Other page\n\nThe other page's text.\n\n## Second part\n\nMore words here.\n\n### Deeper {#deep}\n\nDeep text.\n";

const MARKS: &str = r#"{"image": [400, 200], "marks": {"transport.record": [100, 50, 40, 20], "status.dsp": [300, 180, 80, 16]}}"#;

/// Two actions in two areas: `add_track` (tracks) and `get_stats` (devices).
fn actions() -> Reference {
    Reference {
        areas: [("tracks", "Track actions"), ("devices", "Device actions")]
            .iter()
            .map(|(id, title)| Area {
                id: id.to_string(),
                title: title.to_string(),
                about: format!("About {id}."),
            })
            .collect(),
        actions: [("add_track", "tracks"), ("get_stats", "devices")]
            .iter()
            .map(|(name, area)| Action {
                name: name.to_string(),
                area: area.to_string(),
                description: format!("What {name} does."),
                safety: "edit".to_string(),
                input: Value::Null,
                output: Value::Null,
                ..Action::default()
            })
            .collect(),
        ..Reference::default()
    }
}

fn source_with(contents: &str, pages: &[(&str, &str)]) -> Source {
    let mut source = Source {
        toc: contents.to_string(),
        pages: pages
            .iter()
            .map(|(p, t)| (p.to_string(), t.to_string()))
            .collect(),
        shots: BTreeMap::new(),
        shot_list: None,
        examples: Vec::new(),
    };
    source.shots.insert(
        "sample".to_string(),
        ShotSource {
            png: Some(PathBuf::from("sample.png")),
            marks: Some(MARKS.to_string()),
        },
    );
    source.shots.insert(
        "bare".to_string(),
        ShotSource {
            png: Some(PathBuf::from("bare.png")),
            marks: None,
        },
    );
    source
}

/// The manual with `index` as the index page's text.
fn build_index(index: &str, strict: bool) -> Built {
    let source = source_with(CONTENTS, &[("index.md", index), ("guide/other.md", OTHER)]);
    build(
        &source,
        &Options {
            strict_shots: strict,
            version: "9.8.7".to_string(),
            require_examples: false,
        },
        &actions(),
    )
}

fn errors(built: &Built) -> Vec<&Problem> {
    built
        .problems
        .iter()
        .filter(|p| p.severity == Severity::Error)
        .collect()
}

const SAMPLE: &str = r#"# Index

Copy with [[Ctrl+C]], zoom with [[Ctrl++]]; see [[page:other]], [[page:other#second-part]],
[[page:other#deep|the deep part]], [the same](other.md#second-part), [[page:#tools]] and
[[action:add_track]] or [[action:get_stats|the stats]]. `[[not:a-link]]` stays code.

::: shot sample
highlight transport.record "1" "The **Record** button, or [[R]]."
highlight status.dsp "2"
dim
The transport and the [[page:other|other page]].
:::

## Tools

Text after the figure.
"#;

#[test]
fn a_sample_page_builds_with_every_extension() {
    let built = build_index(SAMPLE, false);
    assert!(built.problems.is_empty(), "{:#?}", built.problems);
    let html = &built.files["index.html"];

    // Keys.
    assert!(html.contains("<span class=\"ox-keys\"><kbd>Ctrl</kbd>+<kbd>C</kbd></span>"));
    assert!(html.contains("<span class=\"ox-keys\"><kbd>Ctrl</kbd>+<kbd>+</kbd></span>"));
    // Page links: the title, a heading's text, given text, a Markdown link, this page.
    assert!(html.contains("<a class=\"ox-link\" href=\"other.html\">Other page</a>"));
    assert!(html.contains("<a class=\"ox-link\" href=\"other.html#second-part\">Second part</a>"));
    assert!(html.contains("<a class=\"ox-link\" href=\"other.html#deep\">the deep part</a>"));
    assert!(html.contains("<a href=\"other.html#second-part\">the same</a>"));
    assert!(html.contains("<a class=\"ox-link\" href=\"#tools\">Tools</a>"));
    // Action links.
    assert!(html.contains(
        "<a class=\"ox-link ox-action\" href=\"mcp-tracks.html#add_track\"><code>add_track</code></a>"
    ));
    assert!(html.contains("href=\"mcp-devices.html#get_stats\">the stats</a>"));
    // Code spans are left alone.
    assert!(html.contains("<code>[[not:a-link]]</code>"));

    // The figure: the image, its size, the dimming mask, the highlights in percentages, the
    // callouts listed with their (Markdown) text, the caption with its link.
    assert!(html.contains("<figure class=\"ox-figure\">"));
    assert!(html.contains(
        "<div class=\"ox-shot\" style=\"aspect-ratio: 400 / 200; max-width: 400px\" data-size=\"400 200\">"
    ));
    assert!(
        html.contains("<img src=\"shots/sample.png\" alt=\"The transport and the other page.\"")
    );
    assert!(html.contains("<svg class=\"ox-dim\" viewBox=\"0 0 400 200\""));
    assert!(html
        .contains("<rect x=\"100\" y=\"50\" width=\"40\" height=\"20\" rx=\"4\" fill=\"black\"/>"));
    assert!(html.contains(
        "<span class=\"ox-hl\" data-mark=\"transport.record\" data-rect=\"100 50 40 20\" style=\"left: 25%; top: 25%; width: 10%; height: 10%\"><span class=\"ox-callout ox-at-left\">1</span></span>"
    ));
    assert!(html.contains(
        "<span class=\"ox-hl\" data-mark=\"status.dsp\" data-rect=\"300 180 80 16\" style=\"left: 75%; top: 90%; width: 20%; height: 8%\"><span class=\"ox-callout ox-at-left\">2</span></span>"
    ));
    assert!(html.contains(
        "<li><span class=\"ox-callout\">1</span> The <strong>Record</strong> button, or <span class=\"ox-keys\"><kbd>R</kbd></span>.</li>"
    ));
    assert!(html.contains(
        "<figcaption><p>The transport and the <a class=\"ox-link\" href=\"other.html\">other page</a>.</p>"
    ));
    // The figure replaced its placeholder entirely, and the text after it is still there.
    assert!(!html.contains("oxide-figure"));
    assert!(!html.contains("-->"));
    assert!(html.contains("<p>Text after the figure.</p>"));

    // Headings get anchors; the page's own contents list them.
    assert!(html.contains("<h2 id=\"tools\">Tools <a class=\"ox-anchor\" href=\"#tools\""));
    assert!(html.contains("<li class=\"ox-toc-2\"><a href=\"#tools\">Tools</a></li>"));
    let other = &built.files["other.html"];
    assert!(other.contains("<h3 id=\"deep\">Deeper"));

    // The frame: the sidebar with this page current, previous/next, the version, the scripts.
    assert!(html.contains("<a href=\"index.html\" aria-current=\"page\">Index</a>"));
    assert!(html.contains("<a href=\"other.html\">Other page</a>"));
    assert!(html.contains(
        "<a class=\"ox-next\" rel=\"next\" href=\"other.html\"><span>Next</span>Other page</a>"
    ));
    assert!(other.contains(
        "<a class=\"ox-prev\" rel=\"prev\" href=\"index.html\"><span>Previous</span>Index</a>"
    ));
    assert!(other.contains("<a class=\"ox-next\" rel=\"next\" href=\"mcp-reference.html\">"));
    assert!(html.contains("This manual documents Oxide 9.8.7."));
    assert!(html.contains("<script src=\"search-index.js\" defer></script>"));
    assert!(html.contains("<script src=\"assets/manual.js\" defer></script>"));
    assert!(!html.contains("type=\"module\""));
    assert!(!html.contains("http://") && !html.contains("https://"));

    // The generated MCP reference (an overview, a page per area), and every file the
    // manual needs.
    let reference = &built.files["mcp-reference.html"];
    assert!(reference.contains("href=\"mcp-tracks.html#add_track\"><code>add_track</code>"));
    let tracks = &built.files["mcp-tracks.html"];
    assert!(tracks.contains("<h2 id=\"add_track\" class=\"ox-action-name\"><code>add_track</code>"));
    assert!(tracks.contains(
        "What <a class=\"ox-link ox-action\" href=\"#add_track\"><code>add_track</code></a> does."
    ));
    assert!(built.files.contains_key("mcp-devices.html"));
    for file in ["search-index.js", "assets/manual.css", "assets/manual.js"] {
        assert!(built.files.contains_key(file), "{file}");
    }
    assert_eq!(
        built.images,
        BTreeMap::from([(
            "shots/sample.png".to_string(),
            ImageJob {
                source: PathBuf::from("sample.png"),
                crop: Some([0, 0, 400, 200]),
                size: Some([400, 200]),
            }
        )])
    );
}

/// A shot `name` with these marks (its PNG never read: only `write` reads images).
fn with_shot(mut source: Source, name: &str, marks: &str) -> Source {
    source.shots.insert(
        name.to_string(),
        ShotSource {
            png: Some(PathBuf::from(format!("{name}.png"))),
            marks: Some(marks.to_string()),
        },
    );
    source
}

fn build_source(source: &Source, strict: bool) -> Built {
    build(
        source,
        &Options {
            strict_shots: strict,
            version: "9.8.7".to_string(),
            require_examples: false,
        },
        &actions(),
    )
}

/// The marks of a 1440x900 window, the app's names.
const WINDOW: &str = r#"{"image": [1440, 900], "marks": {
    "status": [0, 871, 1440, 29],
    "status.device": [8, 878, 318, 15],
    "status.format": [339, 878, 165, 15],
    "status.pdc": [517, 878, 93, 15],
    "status.version": [1385, 878, 47, 15],
    "transport.dsp": [1190, 37, 246, 22]
}}"#;

#[test]
fn a_cropped_figure_with_three_numbered_highlights_renders_exactly() {
    let page = "# Index

::: shot window
crop status.device status.pdc 6
highlight status.device \"1\" \"The device.\"
highlight status.format \"2\" \"The rate.\"
highlight status.pdc \"3\" \"The PDC.\"
dim
The status bar.
:::
";
    let source = with_shot(
        source_with(CONTENTS, &[("index.md", page), ("guide/other.md", OTHER)]),
        "window",
        WINDOW,
    );
    let built = build_source(&source, true);
    assert!(built.problems.is_empty(), "{:#?}", built.problems);
    let html = &built.files["index.html"];
    // The crop: the marks' union (8..610 x 878..893) grown by 6 = 2, 872, 614 x 27, at its
    // own size (narrower than MAX_WIDTH), shown no wider than that. Its badges have no room
    // in a 27-pixel strip: the first stands left of its outline, the other two above theirs
    // (left would cover the outline before), in a margin of 22 pixels on those sides.
    assert!(html.contains(
        "<figure class=\"ox-figure\">\n<div class=\"ox-frame\" style=\"padding: 22px 0px 0px 22px\"><div class=\"ox-shot\" style=\"aspect-ratio: 614 / 27; max-width: 614px\" data-size=\"614 27\"><img src=\"shots/window-2-872-614-27.png\" alt=\"The status bar.\" loading=\"lazy\">"
    ), "{html}");
    // The dim mask in the crop's pixels, a hole per highlight.
    assert!(html.contains("<svg class=\"ox-dim\" viewBox=\"0 0 614 27\""));
    assert!(html
        .contains("<rect x=\"6\" y=\"6\" width=\"318\" height=\"15\" rx=\"4\" fill=\"black\"/>"));
    // Each highlight moved into the crop: (x - 2, y - 872), in percentages of 614 x 27.
    for (mark, rect, style, side, label) in [
        (
            "status.device",
            "6 6 318 15",
            "left: 0.977%; top: 22.222%; width: 51.792%; height: 55.556%",
            "left",
            "1",
        ),
        (
            "status.format",
            "337 6 165 15",
            "left: 54.886%; top: 22.222%; width: 26.873%; height: 55.556%",
            "above",
            "2",
        ),
        (
            "status.pdc",
            "515 6 93 15",
            "left: 83.876%; top: 22.222%; width: 15.147%; height: 55.556%",
            "above",
            "3",
        ),
    ] {
        let span = format!(
            "<span class=\"ox-hl\" data-mark=\"{mark}\" data-rect=\"{rect}\" style=\"{style}\"><span class=\"ox-callout ox-at-{side}\">{label}</span></span>"
        );
        assert!(html.contains(&span), "{span}\n{html}");
    }
    assert!(html.contains("<li><span class=\"ox-callout\">3</span> The PDC.</li>"));
    // The frame closes after the picture, before the caption.
    assert!(
        html.contains("</span></div>\n</div>\n<figcaption>"),
        "{html}"
    );
    // The image to make: that crop of the shot, one file.
    assert_eq!(
        built.images,
        BTreeMap::from([(
            "shots/window-2-872-614-27.png".to_string(),
            ImageJob {
                source: PathBuf::from("window.png"),
                crop: Some([2, 872, 614, 27]),
                size: Some([614, 27]),
            }
        )])
    );
}

#[test]
fn a_highlight_outside_its_crop_fails_and_one_cut_by_it_warns() {
    let figure = |body: &str| {
        format!(
            "# Index

::: shot window
{body}
:::
"
        )
    };
    let build_page = |page: &str| {
        let source = with_shot(
            source_with(CONTENTS, &[("index.md", page), ("guide/other.md", OTHER)]),
            "window",
            WINDOW,
        );
        build_source(&source, true)
    };
    let outside = build_page(&figure(
        "crop status.pdc
highlight transport.dsp \"1\"",
    ));
    let errors = errors(&outside);
    assert_eq!(errors.len(), 1, "{:#?}", outside.problems);
    assert_eq!(errors[0].kind, RefKind::Mark);
    assert_eq!(errors[0].line, 5);
    assert!(errors[0].message.contains("outside the figure's crop"));

    let cut = build_page(&figure(
        "crop 0 860 200 40
highlight status.device \"1\"",
    ));
    assert!(super::tests::errors(&cut).is_empty(), "{:#?}", cut.problems);
    assert_eq!(cut.problems.len(), 1);
    assert_eq!(cut.problems[0].severity, Severity::Warning);
    assert!(cut.problems[0]
        .message
        .contains("cuts into mark \"status.device\""));
    // The outline is cut to the crop: 8..326 becomes 8..200.
    assert!(cut.files["index.html"].contains("data-rect=\"8 18 192 15\""));
}

#[test]
fn a_shot_not_in_the_shot_list_fails() {
    let page = "# Index

::: shot window
The window.
:::
";
    let mut source = with_shot(
        source_with(CONTENTS, &[("index.md", page), ("guide/other.md", OTHER)]),
        "window",
        WINDOW,
    );
    source.shot_list = Some("[shots.other]\nabout = \"Another\"\n".to_string());
    let built = build_source(&source, false);
    let errors = errors(&built);
    assert_eq!(errors.len(), 1, "{:#?}", built.problems);
    assert_eq!(errors[0].kind, RefKind::Shot);
    assert!(errors[0].message.contains("isn't in shots.toml"));
    source.shot_list = Some("[shots.window]\nabout = \"The window\"\n".to_string());
    assert!(build_source(&source, false).problems.is_empty());
    // A list that can't be read is an error of its own.
    source.shot_list = Some("[shots.window]\n".to_string());
    assert_eq!(super::tests::errors(&build_source(&source, false)).len(), 1);
}

/// A marks file the app wrote (`daw --demo .. --screenshot .. --marks ..`, 27.2): the names a
/// highlight may use are the ones the app drew.
const APP_MARKS: &str = include_str!("../tests/fixtures/app/main-window.json");

#[test]
fn a_highlight_naming_a_mark_the_app_never_drew_fails() {
    let page = |mark: &str| {
        format!(
            "# Index

::: shot main-window
crop transport.dsp 8
highlight {mark} \"1\"
:::
"
        )
    };
    let build_page = |mark: &str| {
        let text = page(mark);
        let source = with_shot(
            source_with(CONTENTS, &[("index.md", &text), ("guide/other.md", OTHER)]),
            "main-window",
            APP_MARKS,
        );
        build_source(&source, true)
    };
    // The names the app draws pass...
    let drawn = build_page("transport.dsp");
    assert!(drawn.problems.is_empty(), "{:#?}", drawn.problems);
    // ...a name it never drew (27.1's guess, a typo) fails, naming what it has.
    for wrong in [
        "status.input_gap",
        "transport.dps",
        "settings.processing_block",
    ] {
        let built = build_page(wrong);
        let errors = errors(&built);
        assert_eq!(errors.len(), 1, "{wrong}: {:#?}", built.problems);
        assert_eq!(errors[0].kind, RefKind::Mark);
        assert!(
            errors[0]
                .message
                .contains(&format!("has no mark \"{wrong}\""))
                && errors[0].message.contains("transport.dsp"),
            "{}",
            errors[0].message
        );
    }
}

#[test]
fn a_missing_shot_is_a_placeholder_and_a_warning_until_strict() {
    let page = "# Index\n\n::: shot nowhere\nhighlight transport.record \"1\"\nIts caption.\n:::\n";
    let built = build_index(page, false);
    assert!(errors(&built).is_empty(), "{:#?}", built.problems);
    assert_eq!(built.problems.len(), 1);
    let warning = &built.problems[0];
    assert_eq!(warning.severity, Severity::Warning);
    assert_eq!(warning.kind, RefKind::Shot);
    assert_eq!((warning.file.as_str(), warning.line), ("index.md", 3));
    let html = &built.files["index.html"];
    assert!(html.contains("<div class=\"ox-shot ox-missing\" style=\"aspect-ratio: 16 / 9\">"));
    assert!(html.contains("Screenshot \u{201c}nowhere\u{201d} isn't made yet."));
    assert!(html.contains("<figcaption><p>Its caption.</p>"));
    assert!(built.images.is_empty());

    let strict = build_index(page, true);
    assert_eq!(errors(&strict).len(), 1);
    assert_eq!(errors(&strict)[0].kind, RefKind::Shot);

    // A shot without its marks can't take highlights.
    let bare = build_index("# Index\n\n::: shot bare\nhighlight a.b\n:::\n", false);
    assert_eq!(bare.problems.len(), 1);
    assert!(bare.problems[0].message.contains("no marks"));
    // ... but needs none for a plain picture.
    let plain = build_index("# Index\n\n::: shot bare\nA plain picture.\n:::\n", true);
    assert!(plain.problems.is_empty(), "{:#?}", plain.problems);
}

#[test]
fn each_kind_of_broken_reference_fails() {
    let cases: &[(&str, RefKind, usize)] = &[
        ("[[page:nowhere]]", RefKind::Page, 3),
        ("[x](missing.md)", RefKind::Page, 3),
        ("[[page:other#nope]]", RefKind::Anchor, 3),
        ("[x](other.md#nope)", RefKind::Anchor, 3),
        ("Text.\n\n[[page:#nope]]", RefKind::Anchor, 5),
        ("[[action:make_coffee]]", RefKind::Action, 3),
        ("## Same\n\n## Same", RefKind::DuplicateAnchor, 5),
        ("## A {#x}\n\n## B {#x}", RefKind::DuplicateAnchor, 5),
        (
            "::: shot sample\nhighlight transport.stop \"1\"\n:::",
            RefKind::Mark,
            4,
        ),
        ("::: shot sample\ncrop status.nope\n:::", RefKind::Mark, 3),
        (
            "::: shot sample\ncrop 300 100 200 50\n:::",
            RefKind::Syntax,
            3,
        ),
        ("[[sound:Amplify]]", RefKind::Syntax, 3),
        ("[[effect:Amplify]]", RefKind::Effect, 3),
        ("[[effect:amplify]]", RefKind::Effect, 3),
        (
            "::: params
:::",
            RefKind::Syntax,
            3,
        ),
        ("[[Ctrl+]]", RefKind::Syntax, 3),
        ("::: note\n:::", RefKind::Syntax, 3),
        ("# A second title", RefKind::Syntax, 3),
        (
            "A caption link:\n\n::: shot sample\n[[page:gone]]\n:::",
            RefKind::Page,
            5,
        ),
    ];
    for (text, kind, line) in cases {
        let built = build_index(&format!("# Index\n\n{text}\n"), false);
        let errors = errors(&built);
        assert_eq!(errors.len(), 1, "{text:?}: {:#?}", built.problems);
        assert_eq!(errors[0].kind, *kind, "{text:?}: {}", errors[0]);
        assert_eq!(
            (errors[0].file.as_str(), errors[0].line),
            ("index.md", *line),
            "{text:?}: {}",
            errors[0]
        );
    }
}

#[test]
fn the_contents_and_the_files_must_agree() {
    let pages = [("index.md", "# Index\n"), ("guide/other.md", OTHER)];
    let check = |contents: &str, pages: &[(&str, &str)], what: &str| {
        let built = build(
            &source_with(contents, pages),
            &Options::default(),
            &actions(),
        );
        let errors = errors(&built);
        assert!(
            errors
                .iter()
                .any(|e| e.kind == RefKind::Contents && e.message.contains(what)),
            "{what}: {:#?}",
            built.problems
        );
    };
    // A page that isn't listed.
    check(
        "[[chapter]]\ntitle = \"A\"\npages = [\"index.md\"]\n",
        &pages,
        "isn't in manual.toml",
    );
    // A listed page that isn't there.
    check(
        &format!("{CONTENTS}\n[[chapter]]\ntitle = \"B\"\npages = [\"gone.md\"]\n"),
        &pages,
        "lists \"gone.md\", which isn't there",
    );
    // Two pages with one name.
    check(
        "[[chapter]]\ntitle = \"A\"\npages = [\"index.md\", \"guide/other.md\", \"more/other.md\"]\n",
        &[
            ("index.md", "# Index\n"),
            ("guide/other.md", OTHER),
            ("more/other.md", OTHER),
        ],
        "twice",
    );
    // The index first.
    check(
        "[[chapter]]\ntitle = \"A\"\npages = [\"guide/other.md\", \"index.md\"]\n",
        &pages,
        "first page must be index.md",
    );
    check(
        "[[chapter]]\ntitle = \"A\"\npages = [\"index.md\", \"@nope\", \"guide/other.md\"]\n",
        &pages,
        "isn't a generated page",
    );
    check("[[chapter]]\ntitle = 1\n", &pages, "can't be read");
    // A page without a title.
    let built = build(
        &source_with(
            CONTENTS,
            &[("index.md", "Just text.\n"), ("guide/other.md", OTHER)],
        ),
        &Options::default(),
        &actions(),
    );
    assert!(errors(&built)
        .iter()
        .any(|e| e.kind == RefKind::Syntax && e.message.contains("no title")));
    // Errors name the contents' line.
    let built = build(
        &source_with(
            &format!("{CONTENTS}\n[[chapter]]\ntitle = \"B\"\npages = [\"gone.md\"]\n"),
            &pages,
        ),
        &Options::default(),
        &actions(),
    );
    assert_eq!(errors(&built)[0].line, 12);
}

#[test]
fn the_search_index_holds_titles_headings_and_text() {
    let built = build_index(SAMPLE, false);
    let index = &built.index;
    assert_eq!(index.version, "9.8.7");
    let titles: Vec<&str> = index.pages.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Index",
            "Other page",
            "MCP reference",
            "Track actions",
            "Device actions"
        ]
    );
    assert_eq!(index.pages[1].file, "other.html");
    let other: Vec<(&str, &str)> = index
        .entries
        .iter()
        .filter(|e| e.page == 1)
        .map(|e| (e.anchor.as_str(), e.heading.as_str()))
        .collect();
    assert_eq!(
        other,
        [("", ""), ("second-part", "Second part"), ("deep", "Deeper")]
    );
    let deep = index.entries.iter().find(|e| e.anchor == "deep").unwrap();
    assert_eq!(deep.text, "Deep text.");
    // Figure captions and callouts are searchable, under the section they sit in.
    let top = &index.entries[0];
    assert!(
        top.text.contains("The transport and the other page."),
        "{}",
        top.text
    );
    assert!(
        top.text.contains("The Record button, or R."),
        "{}",
        top.text
    );
    // Actions too.
    assert!(index
        .entries
        .iter()
        .any(|e| e.anchor == "add_track" && e.text == "What add_track does."));
    assert!(built.files["search-index.js"].contains("\"title\":\"Other page\""));

    // Ranking: the page titled with the word, then the heading, then text.
    let file = |hit: &search::Hit| {
        let e = &index.entries[hit.entry];
        format!("{}#{}", index.pages[e.page].file, e.anchor)
    };
    let hits = search::query(index, "other");
    assert_eq!(file(&hits[0]), "other.html#");
    let hits = search::query(index, "second");
    assert_eq!(file(&hits[0]), "other.html#second-part");
    let hits = search::query(index, "deep");
    assert_eq!(file(&hits[0]), "other.html#deep");
}

/// Every built-in effect's `::: params` block, one section each.
fn every_effect(skip: &str) -> String {
    crate::effects::all()
        .effects
        .iter()
        .filter(|e| e.id != skip)
        .map(|e| {
            format!(
                "## {} {{#{}}}

::: params {}
:::

",
                e.name, e.id, e.id
            )
        })
        .collect()
}

#[test]
fn effect_tables_are_made_from_the_effects_and_linked() {
    let page = format!(
        "# Index

See [[effect:amplify]] and [[effect:phaser|the phaser]].

{}",
        every_effect("")
    );
    let built = build_index(&page, false);
    assert!(errors(&built).is_empty(), "{:#?}", built.problems);
    let html = &built.files["index.html"];
    assert!(html.contains("<a class=\"ox-link\" href=\"#amplify\">Amplify</a>"));
    assert!(html.contains("<a class=\"ox-link\" href=\"#phaser\">the phaser</a>"));
    assert!(html.contains("<td>Left Gain</td><td>-96.00 dB to +48.00 dB</td>"));
    assert!(html.contains("Delay: 7193 samples at 48 kHz"));
    // The parameters are searchable under their effect's heading.
    let index = &built.files["search-index.json"];
    assert!(
        index.contains("Link Sliders"),
        "the table's words are indexed"
    );
}

#[test]
fn every_effect_is_documented_once_a_page_documents_one() {
    let missing = build_index(
        &format!(
            "# Index

{}",
            every_effect("phaser")
        ),
        false,
    );
    let errors_found = errors(&missing);
    assert_eq!(errors_found.len(), 1, "{:#?}", missing.problems);
    assert_eq!(errors_found[0].kind, RefKind::Effect);
    assert!(errors_found[0].message.contains("Phaser (phaser)"));

    let twice = build_index(
        &format!(
            "# Index

{}## Again

::: params delay
:::
",
            every_effect("")
        ),
        false,
    );
    let errors_found = errors(&twice);
    assert_eq!(errors_found.len(), 1, "{:#?}", twice.problems);
    assert!(errors_found[0].message.contains("twice"));

    let filled = build_index(
        "# Index

::: params amplify
something
:::
",
        false,
    );
    assert!(errors(&filled)
        .iter()
        .any(|e| e.kind == RefKind::Syntax && e.line == 4));
}
