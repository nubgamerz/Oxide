//! The user guide answers plain questions (dev plan 27.4): each question, typed into the
//! search box as a user would ask it, ranks the page and section that answers it first. Built
//! over the real `docs/manual`, with the same ranking the browser and `search_manual` use.

use std::path::Path;

/// A question and where its answer is: a page id and the section's anchor (empty: the page's
/// top; `*`: the page's top or any of its sections, for a question that names the page's title,
/// whose top then ranks first by design).
const QUESTIONS: &[(&str, &str, &str)] = &[
    // The user's own three examples.
    (
        "monitor while playing",
        "recording",
        "monitoring-while-playing-and-recording",
    ),
    (
        "open the piano roll",
        "piano-roll",
        "opening-the-piano-roll",
    ),
    ("choppy audio high DSP", "choppy-audio", ""),
    (
        "raise the processing block",
        "choppy-audio",
        "raise-the-processing-block",
    ),
    // ... and as the user worded them.
    (
        "how to monitor while playing",
        "recording",
        "monitoring-while-playing-and-recording",
    ),
    (
        "how to enable the piano roll",
        "piano-roll",
        "opening-the-piano-roll",
    ),
    (
        "increase the processing block in case of choppy audio and high DSP",
        "choppy-audio",
        "*",
    ),
    // More plain questions.
    ("count-in", "recording", "the-count-in"),
    (
        "loop recording takes",
        "recording",
        "loop-recording-and-takes",
    ),
    ("no sound", "no-sound", ""),
    ("plug-in missing", "plugin-problems", ""),
    ("Claude can't connect", "mcp-not-connecting", "*"),
    ("buffer size", "audio-device", "buffer-size"),
    ("project folder", "getting-started", "the-project-folder"),
    ("split a clip", "multitrack", "split-a-clip"),
    ("fade in", "multitrack", "fades-and-clip-gain"),
    ("draw notes", "piano-roll", "drawing-and-editing-notes"),
    ("keyboard shortcuts", "keyboard", ""),
    ("punch recording", "recording", "*"),
    ("markers", "multitrack", "markers"),
    ("metronome", "recording", "the-metronome"),
    // The guide's second half (27.5).
    ("add a send", "mixer", "add-a-send"),
    ("pre-fader send", "mixer", "send-level-pre-and-post"),
    (
        "track output routing",
        "mixer",
        "where-a-track-goes-its-output",
    ),
    (
        "bypass a plug-in",
        "mixer",
        "bypass-reorder-copy-and-remove",
    ),
    ("automate the volume", "automation", "automate-the-volume"),
    ("touch latch write", "automation", "automation-modes"),
    ("export an mp3", "exporting", "export-an-mp3"),
    (
        "normalise to -14 LUFS",
        "exporting",
        "normalise-to-a-loudness-target",
    ),
    (
        "normalize loudness",
        "exporting",
        "normalise-to-a-loudness-target",
    ),
    ("dither", "exporting", "dither"),
    ("render a track", "rendering", "*"),
    ("freeze a track", "rendering", "switch-a-track-off-freeze"),
    ("remove clicks", "waveform-view", "diagnostics"),
    ("click remover", "effects-restoration", "click-remover"),
    (
        "find and repair clicks in a file",
        "waveform-view",
        "diagnostics",
    ),
    ("remove noise", "effects-restoration", "noise-reduction"),
    (
        "spectral repair",
        "waveform-view",
        "the-spectrogram-and-spectral-repair",
    ),
    ("restore a backup", "projects", "restore-a-backup"),
    ("save as template", "projects", "save-as-template"),
    ("connect Claude", "settings", "connect-claude"),
    ("autosave", "projects", "autosave-and-backups"),
    (
        "compressor threshold ratio",
        "effects-dynamics",
        "compressor",
    ),
    ("reverb", "effects-reverb", "*"),
    // AI control (27.6).
    ("Claude Desktop when Oxide is closed", "mcp-connecting", "*"),
    (
        "what are the safety classes",
        "mcp-how-actions-work",
        "safety-classes",
    ),
    (
        "ticks and samples units",
        "mcp-how-actions-work",
        "units-samples-ticks-and-bars",
    ),
    ("what does add_track do", "mcp-tracks", "add_track"),
    (
        "write a MIDI part with Claude",
        "mcp-workflows",
        "write-a-midi-part",
    ),
    (
        "poll the export status",
        "mcp-how-actions-work",
        "long-jobs-and-status-actions",
    ),
];

#[test]
fn plain_questions_find_their_answers_first() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual");
    let source = manual::Source::read(&dir, &dir.join("shots")).expect("docs/manual reads");
    let built = manual::build(
        &source,
        &manual::Options::default(),
        &manual::mcp::Reference::load(),
    );
    let index = &built.index;
    let mut wrong = Vec::new();
    for &(question, page, anchor) in QUESTIONS {
        let hits = manual::search::query(index, question);
        let first = hits.first().map(|hit| {
            let entry = &index.entries[hit.entry];
            (index.page_id(entry.page).to_string(), entry.anchor.clone())
        });
        println!("{question:?} -> {first:?}");
        let right = first
            .as_ref()
            .is_some_and(|(p, a)| p == page && (anchor == "*" || a == anchor));
        if !right {
            wrong.push(format!(
                "{question:?}: first {first:?}, wanted {page}#{anchor}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
