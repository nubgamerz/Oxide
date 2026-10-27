//! A VST3 plug-in's parameters, kept on the main thread (dev plan 18.4, TD22).
//!
//! The audio thread only `try_lock`s a plug-in, so anything the main thread does under the
//! plug-in's lock can cost the track a silent block. Parameters never need it: VST3 gives them
//! to the edit controller, which runs on the UI thread while the processor runs on the audio
//! thread. The vendored host's [`ControllerLink`] reaches the controller and the host's queues
//! without the lock, and this cache sits on it:
//!
//! - the list (ids, names, units, steps, defaults, flags) and each value are read at load,
//!   after activation and the restored state, and read again whenever the plug-in says its
//!   titles or values changed (`restartComponent`, [`ParameterCache::refresh`]);
//! - values that move otherwise are noted as they arrive: a set through the cache at once, the
//!   editor's knobs from its gestures, automation and the processor's own outputs from the
//!   link's queues ([`ParameterCache::take_changes`], every frame);
//! - display text is asked of the controller each time ([`ParameterCache::display`]): it is
//!   always the plug-in's own, and costs no lock;
//! - a set goes to the controller now and to the processor at the start of its next block,
//!   through the link's lock-free inbox ([`ParameterCache::set`]).

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use vst3_host::parameters::Parameter;
use vst3_host::{ControllerLink, ParameterEdit, ParameterEditKind};

/// What the cache reads from and writes to: the plug-in's [`ControllerLink`], or a stand-in in
/// tests.
pub trait Controller {
    /// Every parameter, with its value now.
    fn read_all(&self) -> Result<Vec<Parameter>>;
    /// How the plug-in shows `value` for `id`.
    fn format(&self, id: u32, value: f64) -> Result<String>;
    /// `value` to the controller now and to the processor at its next block.
    fn set(&self, id: u32, value: f64) -> Result<()>;
    /// The editor's gestures since the last call.
    fn take_edits(&self) -> Vec<ParameterEdit>;
    /// Values that moved outside the controller's own calls (automation, the processor's
    /// outputs) since the last call.
    fn take_values(&self) -> Vec<(u32, f64)>;
}

impl Controller for ControllerLink {
    fn read_all(&self) -> Result<Vec<Parameter>> {
        self.parameters()
            .map_err(|e| anyhow!("reading parameters: {e}"))
    }
    fn format(&self, id: u32, value: f64) -> Result<String> {
        ControllerLink::format(self, id, value).map_err(|e| anyhow!("{e}"))
    }
    fn set(&self, id: u32, value: f64) -> Result<()> {
        ControllerLink::set(self, id, value).map_err(|e| anyhow!("{e}"))
    }
    fn take_edits(&self) -> Vec<ParameterEdit> {
        ControllerLink::take_edits(self)
    }
    fn take_values(&self) -> Vec<(u32, f64)> {
        ControllerLink::take_values(self)
    }
}

/// An editor gesture, with the knob's value as it began (for a `BeginGesture`: what a lane
/// written from it starts at).
#[derive(Debug, Clone, PartialEq)]
pub struct Gesture {
    pub edit: ParameterEdit,
    pub before: Option<f64>,
}

pub struct ParameterCache<C = ControllerLink> {
    link: C,
    params: Vec<Parameter>,
    index: HashMap<u32, usize>,
}

impl<C: Controller> ParameterCache<C> {
    /// Read everything `link` has. Make it once the plug-in is active and its state restored.
    pub fn new(link: C) -> Result<Self> {
        let mut cache = Self {
            link,
            params: Vec::new(),
            index: HashMap::new(),
        };
        cache.refresh()?;
        Ok(cache)
    }

    /// Read the whole list and every value again: the plug-in changed its titles or values
    /// (`kParamTitlesChanged`, `kParamValuesChanged`), or its state was set.
    pub fn refresh(&mut self) -> Result<()> {
        self.params = self.link.read_all()?;
        self.index = self
            .params
            .iter()
            .enumerate()
            .map(|(k, p)| (p.id, k))
            .collect();
        Ok(())
    }

    /// What the cache reads from.
    pub fn link(&self) -> &C {
        &self.link
    }

    pub fn parameters(&self) -> &[Parameter] {
        &self.params
    }

    pub fn get(&self, id: u32) -> Option<&Parameter> {
        self.index.get(&id).map(|&k| &self.params[k])
    }

    /// A value learned elsewhere. Ids the list doesn't have are ignored.
    fn note(&mut self, id: u32, value: f64) {
        if let Some(&k) = self.index.get(&id) {
            self.params[k].value = value;
        }
    }

    /// How the plug-in shows `value` for `id` ("-6.0 dB"); empty if it can't say.
    pub fn display(&self, id: u32, value: f64) -> String {
        self.link.format(id, value).unwrap_or_default()
    }

    /// Set `id` to `value` (0..1): the cache and the controller now, the processor at the start
    /// of its next block. Returns how the plug-in shows it.
    pub fn set(&mut self, id: u32, value: f64) -> Result<String> {
        if self.get(id).is_none() {
            return Err(anyhow!("the plug-in has no parameter {id}"));
        }
        self.link.set(id, value)?;
        self.note(id, value);
        Ok(self.display(id, value))
    }

    /// Take what moved since the last call: values from automation and the processor (noted),
    /// and the editor's gestures (their values noted), returned in order, each `BeginGesture`
    /// with the knob's value as it began.
    pub fn take_changes(&mut self) -> Vec<Gesture> {
        for (id, value) in self.link.take_values() {
            self.note(id, value);
        }
        let mut gestures = Vec::new();
        for edit in self.link.take_edits() {
            let before = match (&edit.kind, edit.value) {
                (ParameterEditKind::BeginGesture, _) => self.get(edit.id).map(|p| p.value),
                (ParameterEditKind::ValueChange, Some(value)) => {
                    self.note(edit.id, value.clamp(0.0, 1.0));
                    None
                }
                _ => None,
            };
            gestures.push(Gesture { edit, before });
        }
        gestures
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A plug-in's controller and its queues, in memory.
    #[derive(Default)]
    struct Fake {
        /// (id, title, value)
        params: RefCell<Vec<(u32, String, f64)>>,
        /// What reached the processor's inbox.
        inbox: RefCell<Vec<(u32, f64)>>,
        edits: RefCell<Vec<ParameterEdit>>,
        values: RefCell<Vec<(u32, f64)>>,
        reads: RefCell<usize>,
    }

    impl Fake {
        fn with(params: &[(u32, &str, f64)]) -> Self {
            let fake = Fake::default();
            *fake.params.borrow_mut() = params
                .iter()
                .map(|&(id, name, value)| (id, name.to_string(), value))
                .collect();
            fake
        }
    }

    impl Controller for &Fake {
        fn read_all(&self) -> Result<Vec<Parameter>> {
            *self.reads.borrow_mut() += 1;
            Ok(self
                .params
                .borrow()
                .iter()
                .map(|(id, name, value)| Parameter {
                    id: *id,
                    name: name.clone(),
                    value: *value,
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    unit: "dB".into(),
                    step_count: 0,
                    can_automate: true,
                    is_read_only: false,
                    is_bypass: false,
                    flags: 1,
                })
                .collect())
        }
        fn format(&self, id: u32, value: f64) -> Result<String> {
            Ok(format!("{id}: {:.1} dB", value * 10.0))
        }
        fn set(&self, id: u32, value: f64) -> Result<()> {
            if let Some(p) = self.params.borrow_mut().iter_mut().find(|p| p.0 == id) {
                p.2 = value;
            }
            self.inbox.borrow_mut().push((id, value));
            Ok(())
        }
        fn take_edits(&self) -> Vec<ParameterEdit> {
            std::mem::take(&mut self.edits.borrow_mut())
        }
        fn take_values(&self) -> Vec<(u32, f64)> {
            std::mem::take(&mut self.values.borrow_mut())
        }
    }

    fn names<C: Controller>(cache: &ParameterCache<C>) -> Vec<String> {
        cache.parameters().iter().map(|p| p.name.clone()).collect()
    }

    #[test]
    fn reads_once_and_again_only_when_the_titles_change() {
        let fake = Fake::with(&[(1, "Gain", 0.5), (2, "Mix", 1.0)]);
        let mut cache = ParameterCache::new(&fake).unwrap();
        assert_eq!(names(&cache), ["Gain", "Mix"]);
        // Reading the cache asks the plug-in nothing.
        for _ in 0..10 {
            assert_eq!(cache.get(2).unwrap().value, 1.0);
            assert_eq!(cache.parameters().len(), 2);
        }
        assert_eq!(*fake.reads.borrow(), 1);
        // The plug-in renames and adds (a sampler loading an instrument): the cache keeps the old
        // list until told (`kParamTitlesChanged`), then has the new one.
        *fake.params.borrow_mut() = vec![
            (1, "Input Gain".into(), 0.5),
            (2, "Mix".into(), 1.0),
            (7, "Ceiling".into(), 0.9),
        ];
        assert_eq!(names(&cache), ["Gain", "Mix"]);
        cache.refresh().unwrap();
        assert_eq!(names(&cache), ["Input Gain", "Mix", "Ceiling"]);
        assert_eq!(cache.get(7).unwrap().value, 0.9);
        assert_eq!(*fake.reads.borrow(), 2);
    }

    #[test]
    fn a_state_set_is_read_again() {
        let fake = Fake::with(&[(1, "Gain", 0.5), (2, "Mix", 1.0)]);
        let mut cache = ParameterCache::new(&fake).unwrap();
        // A state (a preset, an undo) moves every value at once in the controller.
        for p in fake.params.borrow_mut().iter_mut() {
            p.2 = 0.25;
        }
        assert_eq!(cache.get(1).unwrap().value, 0.5, "not until refreshed");
        cache.refresh().unwrap();
        assert_eq!(cache.get(1).unwrap().value, 0.25);
        assert_eq!(cache.get(2).unwrap().value, 0.25);
    }

    #[test]
    fn a_set_is_queued_for_the_processor_and_shown_at_once() {
        let fake = Fake::with(&[(1, "Gain", 0.5), (2, "Mix", 1.0)]);
        let mut cache = ParameterCache::new(&fake).unwrap();
        assert_eq!(cache.set(1, 0.8).unwrap(), "1: 8.0 dB");
        assert_eq!(cache.set(1, 0.6).unwrap(), "1: 6.0 dB");
        // Both reached the processor's inbox, in order (the last wins at its next block), and
        // the cache has the newest without reading the plug-in again.
        assert_eq!(*fake.inbox.borrow(), [(1, 0.8), (1, 0.6)]);
        assert_eq!(cache.get(1).unwrap().value, 0.6);
        assert_eq!(*fake.reads.borrow(), 1);
        // An id the plug-in doesn't have is refused before anything is queued.
        assert!(cache.set(9, 0.1).is_err());
        assert_eq!(fake.inbox.borrow().len(), 2);
    }

    #[test]
    fn values_from_automation_and_gestures_are_noted_in_order() {
        let fake = Fake::with(&[(1, "Gain", 0.5), (2, "Mix", 1.0)]);
        let mut cache = ParameterCache::new(&fake).unwrap();
        fake.values
            .borrow_mut()
            .extend([(2, 0.4), (2, 0.3), (99, 0.1)]);
        let edit = |kind, value| ParameterEdit { id: 1, kind, value };
        fake.edits.borrow_mut().extend([
            edit(ParameterEditKind::BeginGesture, None),
            edit(ParameterEditKind::ValueChange, Some(0.7)),
            edit(ParameterEditKind::ValueChange, Some(0.9)),
            edit(ParameterEditKind::EndGesture, None),
            edit(ParameterEditKind::BeginGesture, None),
        ]);
        let gestures = cache.take_changes();
        assert_eq!(
            cache.get(2).unwrap().value,
            0.3,
            "the newest automation value"
        );
        assert_eq!(
            cache.get(1).unwrap().value,
            0.9,
            "the knob where it was let go"
        );
        assert_eq!(gestures.len(), 5);
        assert_eq!(
            gestures[0].before,
            Some(0.5),
            "the knob as the first gesture began"
        );
        assert_eq!(gestures[4].before, Some(0.9), "and as the second did");
        assert!(gestures[1..4].iter().all(|g| g.before.is_none()));
        assert!(cache.take_changes().is_empty(), "taken once");
    }
}
