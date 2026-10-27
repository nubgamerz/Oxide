//! The metronome: a synthesized click on every beat, louder and higher on the downbeat.
//!
//! It is mixed into the hardware output after the master, so no plug-in or fader touches it,
//! and only in the live callback: export never runs it, so a bounce can never contain it.
//!
//! Clicks follow what you *hear*, not the playhead. The output runs the graph's delay
//! compensation behind the timeline, so the metronome keeps its own "heard" position: the
//! playhead minus that latency. A loop wrap reaches the ears one latency after the engine
//! jumps, so the jump is queued until then.

use crate::tempo::TempoMap;

/// What the engine needs to know about the metronome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClickSettings {
    pub enabled: bool,
    /// Linear gain of the click.
    pub gain: f32,
    pub only_while_recording: bool,
    /// First channel of the output pair it plays on (the master's).
    pub channel: usize,
    /// Muting the master mutes the click too.
    pub muted: bool,
}

impl Default for ClickSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            gain: 0.5,
            only_while_recording: false,
            channel: 0,
            muted: false,
        }
    }
}

/// Length of one click, in seconds.
const CLICK_SECONDS: f64 = 0.03;
/// Loop wraps that can be on their way to the output at once (a loop shorter than the
/// latency has more than one).
const MAX_JUMPS: usize = 4;

pub struct Metronome {
    pub settings: ClickSettings,
    /// Timeline position of the next output sample.
    heard: i64,
    /// Output samples produced so far.
    clock: u64,
    /// Heard-position jumps still on their way: at output sample `.0`, `heard` becomes `.1`.
    jumps: [Option<(u64, i64)>; MAX_JUMPS],
    /// The next beat to click, by index from the timeline start, and the sample it falls on.
    next_beat: i64,
    due: i64,
    /// `next_beat` must be worked out again (after a jump or a tempo change).
    stale: bool,
    /// The click sounding now: downbeat or not, and samples into it.
    voice: Option<(bool, usize)>,
}

impl Default for Metronome {
    fn default() -> Self {
        Self::new()
    }
}

impl Metronome {
    pub fn new() -> Self {
        Self {
            settings: ClickSettings::default(),
            heard: 0,
            clock: 0,
            jumps: [None; MAX_JUMPS],
            next_beat: 0,
            due: 0,
            stale: true,
            voice: None,
        }
    }

    /// Timeline position of the audio leaving the output now.
    pub fn heard(&self) -> i64 {
        self.heard
    }

    /// The output now plays the timeline from `playhead − latency` on: after a locate, at
    /// the start of playback, or when the latency changed. The playhead is negative during a
    /// count-in that starts before the timeline.
    pub fn sync(&mut self, playhead: i64, latency: usize) {
        self.heard = playhead - latency as i64;
        self.jumps = [None; MAX_JUMPS];
        self.stale = true;
    }

    /// The tempo map changed: beats fall elsewhere now.
    pub fn tempo_changed(&mut self) {
        self.stale = true;
    }

    /// The engine just jumped to `start`; the output follows `latency` samples from now.
    pub fn wrapped(&mut self, start: u64, latency: usize) {
        let at = self.clock + latency as u64;
        let jump = Some((at, start as i64));
        match self.jumps.iter_mut().find(|j| j.is_none()) {
            Some(slot) => *slot = jump,
            None => {
                // More wraps in flight than slots: the loop is far shorter than the latency.
                // Give up on exactness and follow the playhead from here.
                self.jumps = [None; MAX_JUMPS];
                self.jumps[0] = jump;
            }
        }
    }

    /// Whether a click starts now. A count-in clicks until the record point even with the
    /// metronome off; muting the master silences everything.
    fn sounding(&self, recording: bool, count_in_to: Option<i64>) -> bool {
        let s = &self.settings;
        let counting_in = count_in_to.is_some_and(|to| self.heard < to);
        !s.muted && (counting_in || (s.enabled && (recording || !s.only_while_recording)))
    }

    /// Point `next_beat` at the first beat at or after the heard position. Beats are found by
    /// index on the map, never by adding up lengths, so nothing drifts.
    fn retarget(&mut self, map: &TempoMap) {
        let at = |n: i64| map.sample_of_beat(n as f64).round() as i64;
        let mut n = map.beats_at(self.heard as f64).ceil() as i64;
        // Beats sit on rounded samples; step back if rounding put one at `heard`.
        if at(n - 1) >= self.heard {
            n -= 1;
        }
        self.next_beat = n;
        self.due = at(n);
        self.stale = false;
    }

    /// Add this block's clicks to `output`. Call once per processed block with the playhead
    /// state the block ran with; `count_in_to` is the record point (heard position) while
    /// counting in. Allocation-free.
    pub fn process(
        &mut self,
        output: &mut [Vec<f32>],
        frames: usize,
        map: &TempoMap,
        playing: bool,
        recording: bool,
        count_in_to: Option<i64>,
    ) {
        if self.stale {
            self.retarget(map);
        }
        let rate = f64::from(map.sample_rate.max(1));
        let click_len = (CLICK_SECONDS * rate) as usize;
        let gain = self.settings.gain;
        let (left, right) = (self.settings.channel, self.settings.channel + 1);
        for k in 0..frames {
            if playing {
                for i in 0..MAX_JUMPS {
                    if let Some((at, to)) = self.jumps[i] {
                        if at <= self.clock {
                            self.heard = to;
                            self.jumps[i] = None;
                            self.retarget(map);
                        }
                    }
                }
                if self.heard >= self.due {
                    if self.heard == self.due && self.sounding(recording, count_in_to) {
                        let downbeat = map.beat_in_bar(self.next_beat) == 0;
                        self.voice = Some((downbeat, 0));
                    }
                    self.next_beat += 1;
                    self.due = map.sample_of_beat(self.next_beat as f64).round() as i64;
                }
            }
            if let Some((accent, t)) = self.voice {
                let sample = click(accent, t, rate) * gain;
                for c in [left, right] {
                    if let Some(out) = output.get_mut(c) {
                        out[k] += sample;
                    }
                }
                self.voice = (t + 1 < click_len).then_some((accent, t + 1));
            }
            if playing {
                self.heard += 1;
            }
            self.clock += 1;
        }
    }
}

/// One sample of a click: a short sine with a fast decay. The downbeat is higher and louder.
fn click(accent: bool, t: usize, rate: f64) -> f32 {
    let (freq, level) = if accent { (1600.0, 1.0) } else { (1000.0, 0.6) };
    let time = t as f64 / rate;
    // A 0.5 ms fade-in so the click starts without a step.
    let attack = (time / 0.0005).min(1.0);
    let decay = (-time / 0.006).exp();
    (level * attack * decay * (std::f64::consts::TAU * freq * time).sin()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_map() -> TempoMap {
        TempoMap::constant(48_000, 120.0, 4, 4)
    }

    fn on() -> Metronome {
        let mut m = Metronome::new();
        m.settings.enabled = true;
        m.settings.gain = 1.0;
        m
    }

    /// Run `frames` samples in blocks of `block`, returning the left channel.
    fn run(m: &mut Metronome, meter: &TempoMap, frames: usize, block: usize) -> Vec<f32> {
        let mut all = Vec::new();
        while all.len() < frames {
            let n = block.min(frames - all.len());
            let mut out = vec![vec![0.0; n]; 2];
            m.process(&mut out, n, meter, true, false, None);
            all.extend_from_slice(&out[0]);
        }
        all
    }

    /// Where clicks start: the first non-zero sample after silence (a click has a zero
    /// first sample, so the onset is the sample before the first non-zero one).
    fn onsets(signal: &[f32]) -> Vec<(usize, f32)> {
        let mut found = Vec::new();
        let mut quiet = usize::MAX / 2;
        for (i, s) in signal.iter().enumerate() {
            if *s == 0.0 {
                quiet += 1;
                continue;
            }
            if quiet > 100 {
                let peak = signal[i..(i + 200).min(signal.len())]
                    .iter()
                    .fold(0.0f32, |m, v| m.max(v.abs()));
                found.push((i - 1, peak));
            }
            quiet = 0;
        }
        found
    }

    #[test]
    fn clicks_land_on_exact_beats_with_an_accented_downbeat() {
        // 70 BPM at 48 kHz: 41142.857 samples a beat, so rounding is exercised.
        let meter = TempoMap::constant(48_000, 70.0, 3, 4);
        let beat = 48_000.0 * 60.0 / 70.0;
        let mut m = on();
        m.sync(0, 0);
        let out = run(&mut m, &meter, (beat * 100.0) as usize + 10, 333);
        let found = onsets(&out);
        assert_eq!(found.len(), 101);
        for (n, (at, peak)) in found.iter().enumerate() {
            assert_eq!(*at as i64, (n as f64 * beat).round() as i64, "beat {n}");
            let downbeat = n % 3 == 0;
            assert_eq!(*peak > 0.8, downbeat, "beat {n} peak {peak}");
        }
    }

    #[test]
    fn clicks_follow_tempo_and_meter_changes() {
        // One 4/4 bar at 120, then 3/4 at 90 from bar 1.
        let mut project = crate::project::Project::default();
        project.tempo_changes.push(crate::project::TempoChange {
            bar: 1,
            beat: 0,
            tempo: Some(90.0),
            time_signature: Some((3, 4)),
        });
        let map = TempoMap::of(&project, 48_000);
        let mut m = on();
        m.sync(0, 0);
        let out = run(&mut m, &map, 96_000 + 7 * 32_000 + 10, 333);
        let found = onsets(&out);
        let at: Vec<usize> = found.iter().map(|(a, _)| *a).collect();
        let mut expected: Vec<usize> = (0..4).map(|k| k * 24_000).collect();
        expected.extend((0..8).map(|k| 96_000 + k * 32_000));
        assert_eq!(at, expected);
        // Downbeats: the 4/4 bar's first beat, then every third beat in 3/4.
        let accents: Vec<bool> = found.iter().map(|(_, peak)| *peak > 0.8).collect();
        let mut downbeats = vec![true, false, false, false];
        downbeats.extend([true, false, false, true, false, false, true, false]);
        assert_eq!(accents, downbeats);
    }

    #[test]
    fn clicks_follow_the_heard_position_through_latency_and_loop_wraps() {
        let meter = default_map(); // 120 BPM: a beat is 24000 samples
        let latency = 300;
        let mut m = on();
        // Play from 0: the output is 300 samples behind, so the first beat sounds at 300.
        m.sync(0, latency);
        let mut out = run(&mut m, &meter, 24_000 + latency + 10, 64);
        // The engine loops 0..30000: it wraps after 30000 samples of playback. Feed the
        // jump at that moment; the output follows 300 samples later.
        out.extend(run(&mut m, &meter, 30_000 - out.len(), 64));
        m.wrapped(0, latency);
        out.extend(run(&mut m, &meter, 26_000, 64));
        let at: Vec<usize> = onsets(&out).iter().map(|(a, _)| *a).collect();
        // Beats 0 and 1 (at 300 and 24300), then beat 0 again 300 after the wrap at 30000.
        assert_eq!(at, vec![300, 24_300, 30_300, 54_300]);
    }

    #[test]
    fn a_count_in_clicks_up_to_the_record_point_with_the_metronome_off() {
        let meter = default_map(); // 120 BPM, 4/4: a bar is 96000 samples
        let mut m = Metronome::new();
        m.settings.gain = 1.0;
        // One bar of count-in before a record point at 0: the playhead starts at -96000.
        m.sync(-96_000, 0);
        let mut out = Vec::new();
        while out.len() < 150_000 {
            let mut buf = vec![vec![0.0; 500]; 2];
            m.process(&mut buf, 500, &meter, true, true, Some(0));
            out.extend_from_slice(&buf[0]);
        }
        let found = onsets(&out);
        let at: Vec<usize> = found.iter().map(|(a, _)| *a).collect();
        assert_eq!(
            at,
            vec![0, 24_000, 48_000, 72_000],
            "four clicks, none at the record point"
        );
        assert!(
            found[0].1 > 0.8 && found[1].1 < 0.8,
            "the count-in's downbeat is accented"
        );
    }

    #[test]
    fn only_while_recording_and_mute() {
        let meter = default_map();
        let mut m = on();
        m.settings.only_while_recording = true;
        m.sync(0, 0);
        let mut out = vec![vec![0.0; 64]; 2];
        m.process(&mut out, 64, &meter, true, false, None);
        assert!(out[0].iter().all(|s| *s == 0.0), "not recording");
        m.sync(0, 0);
        m.process(&mut out, 64, &meter, true, true, None);
        assert!(out[0].iter().any(|s| *s != 0.0), "recording");

        let mut m = on();
        m.settings.muted = true;
        m.sync(0, 0);
        let mut out = vec![vec![0.0; 64]; 2];
        m.process(&mut out, 64, &meter, true, false, None);
        assert!(out[0].iter().all(|s| *s == 0.0), "the master is muted");
    }

    #[test]
    fn plays_on_its_channel_pair_only() {
        let mut m = on();
        m.settings.channel = 2;
        m.sync(0, 0);
        let mut out = vec![vec![0.0; 64]; 4];
        m.process(&mut out, 64, &default_map(), true, false, None);
        assert!(out[0].iter().chain(&out[1]).all(|s| *s == 0.0));
        assert!(out[2].iter().any(|s| *s != 0.0) && out[2] == out[3]);
        // A pair past the device's outputs is skipped, not a panic.
        m.settings.channel = 6;
        m.sync(0, 0);
        m.process(&mut out, 64, &default_map(), true, false, None);
    }
}
