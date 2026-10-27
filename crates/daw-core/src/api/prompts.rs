//! MCP prompts: short guides a client can offer its user ("write a part"), each a message
//! that walks an agent through the actions for a job. The app's server publishes them with
//! the tools; `docs/mcp-tools.md` lists them.

use serde_json::{Map, Value};

/// One argument a prompt takes.
pub struct PromptArg {
    pub name: &'static str,
    pub description: &'static str,
    pub required: bool,
}

/// A prompt: its name, what it is for, its arguments and how its message is made.
pub struct PromptSpec {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub arguments: &'static [PromptArg],
    text: fn(&Map<String, Value>) -> String,
}

impl PromptSpec {
    /// The prompt's message for `args` (missing ones take their defaults).
    pub fn text(&self, args: &Map<String, Value>) -> String {
        (self.text)(args)
    }
}

/// Every prompt the server offers.
pub fn prompts() -> &'static [PromptSpec] {
    &PROMPTS
}

/// The prompt called `name`.
pub fn find(name: &str) -> Option<&'static PromptSpec> {
    PROMPTS.iter().find(|p| p.name == name)
}

static PROMPTS: [PromptSpec; 1] = [PromptSpec {
    name: "write_midi_part",
    title: "Write a MIDI part",
    description: "How to write a new MIDI part (a melody, a lead, a counter-line, chords) \
                  that fits a clip already in the song: read it as music, make a track with \
                  an instrument, write the notes in bars and note names, and listen.",
    arguments: &[
        PromptArg {
            name: "clip",
            description: "The id of the MIDI clip to write against (a bass line, chords); \
                          see get_project",
            required: false,
        },
        PromptArg {
            name: "part",
            description: "What to write: \"a lead melody\" (the default), \"a counter-line\", \
                          \"pad chords\"…",
            required: false,
        },
    ],
    text: write_midi_part,
}];

fn write_midi_part(args: &Map<String, Value>) -> String {
    let arg = |name: &str| {
        args.get(name).and_then(|v| match v {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    };
    let part = arg("part").unwrap_or_else(|| "a lead melody".into());
    let against = arg("clip").map_or_else(
        || "the MIDI clip it should fit (find it in get_project)".to_string(),
        |clip| format!("MIDI clip {clip}"),
    );
    format!(
        "Write {part} in Oxide that fits {against}.\n\
         \n\
         1. Read the song. get_project lists the tracks with their MIDI clips (ids, start and \
         length in ticks, 960 per quarter note), the tempo and time signature; get_tempo_map \
         gives every change. describe_midi_clip on the clip gives its key (with the runner-up \
         and their scores), scale, chords bar by bar (`progression`, `chords` with their \
         tones), the tempo and meter where it plays, its range, and every note's song \
         position `at` (\"bar:beat:tick\", from 1) and name (C4 = 60).\n\
         2. Plan the part over the same bars: keep to the key's scale, put chord tones on \
         the strong beats and passing notes between, phrase in two- or four-bar units, and \
         keep its register clear of the clip's range (a melody an octave or more above a bass \
         line). If the key's score is close to the runner-up's, let the chords decide.\n\
         3. Make a track for it: insert_midi_track with `bar` and `bars` (the clip's span), \
         `after` (the clip's track) and `instrument` (an installed instrument from \
         list_plugins); it returns the track and an empty clip over those bars. Or add_track \
         (kind \"instrument\"), add_insert for the instrument, then add_midi_clip with `bar` \
         and `bars`.\n\
         4. Write the notes with add_notes: `at` as \"bar:beat:tick\" (\"5:1:000\", \
         \"5:2:480\"), `pitch` as a name (\"E5\", \"Bb4\"), `length` in ticks (960 a quarter \
         note, 480 an eighth, 1920 a half), `velocity` 1-127 for accents. Every call is one \
         undo step.\n\
         5. Check it: describe_midi_clip on the new clip (its notes, key and chords should \
         agree with the clip's), then set_loop over the bars, locate and play to listen with \
         the user, or render_track / export_midi_clip. Change notes with set_notes or \
         remove_notes, or undo."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Registry;
    use serde_json::json;

    /// The prompt names only actions that exist, and fills in its arguments.
    #[test]
    fn the_part_writing_prompt_names_real_actions_and_its_arguments() {
        let registry = Registry::new();
        let names: Vec<&str> = registry.specs().iter().map(|s| s.name).collect();
        let prompt = find("write_midi_part").expect("listed");
        let text = prompt.text(&Map::new());
        assert!(text.starts_with("Write a lead melody in Oxide that fits the MIDI clip"));
        for action in [
            "get_project",
            "get_tempo_map",
            "describe_midi_clip",
            "insert_midi_track",
            "list_plugins",
            "add_track",
            "add_insert",
            "add_midi_clip",
            "add_notes",
            "set_loop",
            "locate",
            "play",
            "render_track",
            "export_midi_clip",
            "set_notes",
            "remove_notes",
            "undo",
        ] {
            assert!(names.contains(&action), "{action} is not an action");
            assert!(text.contains(action), "{action} is not in the prompt");
        }
        let args = json!({"clip": 42, "part": "a counter-line"});
        let text = prompt.text(args.as_object().unwrap());
        assert!(text.starts_with("Write a counter-line in Oxide that fits MIDI clip 42."));
        assert_eq!(prompts().len(), 1);
    }
}
