//! What only the app window can do: show and capture what the user sees.

use super::NoArgs;
use crate::api::{Registry, Safety, ViewChange};

pub(super) fn register(r: &mut Registry) {
    r.add_deferred(
        "screenshot",
        "Capture the DAW window as a PNG image, to see what the user sees. Plug-in editor \
         windows are separate and not included. Fails while the window is minimised.",
        Safety::Read,
        |host, _: NoArgs, reply| host.screenshot(reply),
    );
    r.add(
        "get_meters",
        "Current meter levels as the UI shows them: per track and for the master, the peak \
         (falling at 24 dB/s) and peak-hold of the left and right channels in dBFS (-100 means \
         silence), and the hardware input and output levels (`inputs_db`, `outputs_db`; the \
         outputs include the metronome, which plays after the master).",
        Safety::Read,
        |host, _: NoArgs| host.meters(),
    );
    r.add(
        "get_view",
        "What the UI shows: the selected track, clip or master, the visible timeline range \
         (samples) and zoom (`samples_per_pixel`), the waveform display gain \
         (`waveform_scale`), which panels are open (`piano_roll`: the MIDI clip open in the \
         editor, if any), whether snap and follow-playhead are on, the song's grid (`grid`: a \
         division or \"auto\", see set_grid) and the division the Multitrack shows now \
         (`grid_division`, Auto's at this zoom), the loudness meter's scale and units \
         (`loudness_scale`, `loudness_relative`), and the Waveform view's time display \
         (`editor_time`) with the grid division it shows in bars and beats \
         (`editor_grid_division`). Track heights are in `get_project`.",
        Safety::Read,
        |host, _: NoArgs| host.view(),
    );
    r.add(
        "set_view",
        "Change what the UI shows: select a track, clip or the master; scroll and zoom the \
         timeline to `start`..`end` (samples; as close as 32 points a sample, where single \
         samples show as dots on a line, and as wide as 2 seconds a point), or `zoom` in, out, \
         to fit the project or to a clip; set every track's row height (`track_height`, points 32 to 400); set the \
         waveform display gain (`waveform_scale`, 1 to 16, display only); open a MIDI clip \
         in the piano roll (`piano_roll`: its id, or 0 to close); show or hide the mixer and \
         inspector; turn snap to the grid (its division: set_grid) and follow-playhead on or \
         off; set the loudness meter's scale (`loudness_scale`: \"ebu9\", -18 to +9 LU around \
         the target, or \"ebu18\", -36 to +18) and units (`loudness_relative`: LU from the \
         target, or LUFS); set the Waveform view's time display (`editor_time`: `time`, \
         `samples` or `bars_beats`). `select_clip` takes audio and MIDI clips. Only the fields given change. View changes are never undo \
         steps. Returns the new view.",
        Safety::Edit,
        |host, change: ViewChange| host.set_view(change),
    );
    r.add(
        "get_layout",
        "The window's layout: where the panes are (`multitrack`, `mixer`, `inspector`, `piano_roll`, `diagnostics`, and `waveform`: an open file's tab, with its `document`, or the place files open while none is), as nested containers (`horizontal` and `vertical` splits with each child's share, `tabs` with the active one) and whether each shows; and the saved layouts' names (`layouts`, \"Default\" first). Users drag tabs to rearrange them.",
        Safety::Read,
        |host, _: NoArgs| host.layout(),
    );
    r.add(
        "set_layout",
        "Arrange the window as the saved layout `name` (get_layout's `layouts`; \"Default\" is the built-in one): its panes where it has them, the mixer, inspector and Diagnostics shown or hidden as it has them; open files go where it keeps their place. Returns the layout.",
        Safety::Edit,
        |host, input: LayoutName| host.set_layout(&input.name),
    );
    r.add(
        "save_layout",
        "Keep the window's layout as `name` (app-wide, in the settings), replacing a saved layout of that name: where each pane is and how big, and where files open (not the open files themselves). Returns the saved names.",
        Safety::Edit,
        |host, input: LayoutName| host.save_layout(&input.name),
    );
    r.add(
        "delete_layout",
        "Delete the saved layout `name`. Returns the saved names.",
        Safety::Destructive,
        |host, input: LayoutName| host.delete_layout(&input.name),
    );
    r.add(
        "list_recent_projects",
        "The start screen's recent projects (app-wide, in the settings), most recently opened \
         first: each one's `path` (absolute, for open_project), `name`, `folder`, `state` \
         (\"present\", \"missing\" when the file is gone or its drive isn't connected, or \
         \"unknown\" when its drive didn't answer in time), `modified` (when it was last \
         saved, RFC 3339, local time) with `saved` as the screen says it (\"today 14:32\", \
         \"yesterday\", \"3 days ago\", a date), the `template` it was started from when Oxide \
         made it from one, and `open` for the project open now.",
        Safety::Read,
        |host, _: NoArgs| host.recent_projects(),
    );
    r.add(
        "get_app_info",
        "What Help > About shows: Oxide's `version` (major.minor.patch, the one its installer \
         compares to refuse a downgrade), the `commit` it was built from (a short git hash, \
         `-modified` if built from changed files; null without the app window) and the \
         program's `folder` (absolute; its settings, templates and rack presets are kept \
         there; null without the app window).",
        Safety::Read,
        |host, _: NoArgs| host.app_info(),
    );
    r.add(
        "remove_recent_project",
        "Take a project off the start screen's recent projects (the list only: the project's \
         files stay as they are). `path`: its absolute path as list_recent_projects gives it. \
         Returns the list left.",
        Safety::Destructive,
        |host, input: RecentPath| host.remove_recent_project(&input.path),
    );
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RecentPath {
    /// The project file's absolute path, as list_recent_projects gives it.
    path: std::path::PathBuf,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct LayoutName {
    /// The layout's name.
    name: String,
}
