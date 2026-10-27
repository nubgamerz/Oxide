//! The DAW engine. The UI drives everything through [`session::Session`].

pub mod analysis;
pub mod api;
pub mod audio_edit;
pub mod automation;
pub mod automation_write;
pub mod cpu;
pub mod deadline;
pub mod device;
pub mod diagnostics;
pub mod encode;
pub mod engine;
pub mod export;
pub mod folder;
pub mod graph;
pub mod grid;
pub mod history;
pub mod loudness;
pub mod manual;
pub mod media;
pub mod metadata;
pub mod metronome;
pub mod midi;
pub mod midi_analysis;
pub mod midi_clip;
pub mod midi_record;
pub mod native;
pub mod param_cache;
pub mod peaks;
pub mod plugins;
pub mod pool;
pub mod project;
pub mod rack;
pub mod record;
pub mod sequencer;
pub mod session;
pub mod smf;
pub mod spectral_edit;
pub mod spectrum;
pub mod stretch;
pub mod template;
pub mod tempo;
pub mod trace;
pub mod usage;
pub mod util;
pub mod wav;

/// A real recording for the tests that measure on one (the `#[ignore]`d cost benches):
/// `OXIDE_TEST_AUDIO` (an absolute path), else `out/test-audio.wav` in the repository. Neither
/// comes with the repository; without it those tests skip that part.
#[cfg(test)]
pub(crate) fn test_audio() -> std::path::PathBuf {
    std::env::var_os("OXIDE_TEST_AUDIO")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/test-audio.wav")
        })
}
