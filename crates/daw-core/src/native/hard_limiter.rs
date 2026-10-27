//! Limiter: a look-ahead peak limiter, all in f32:
//!
//! - the audio is boosted, then delayed a fixed 20 ms (the latency, whatever the look-ahead);
//! - the look-ahead window is the next N samples to be played; its peak (a sliding maximum,
//!   the oldest of equal peaks) drives an envelope that ramps up linearly in amplitude so it
//!   reaches the peak as the peak plays, and falls to 15 % in the release time when nothing
//!   louder is ahead;
//! - the gain is ceiling over envelope above the ceiling; Link Channels shares the louder
//!   channel's; a final clip keeps every sample within the ceiling. Sample peak, no
//!   oversampling: inter-sample peaks can pass it.
//!
//! When the window's peak has played, the next is the oldest of the window's loudest samples.
//! A queue of candidates ([`Peaks`]) keeps it as the window slides, so finding it costs no scan
//! of the window: on silence (or any level held flat) the oldest peak plays every sample, and a
//! scan each time cost the limiter 60 to 80 times its price on music (dev plan 18.1). The
//! queue picks exactly the sample the scan did. The envelope is flushed under
//! [`super::TINY`]: it only ever matters above the ceiling.

use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;

const CEILING: usize = 0;
const BOOST: usize = 1;
const LOOK_AHEAD: usize = 2;
const RELEASE: usize = 3;
const LINK: usize = 4;

const DB: ParamSpec = ParamSpec {
    name: "",
    unit: "dB",
    min: -100.0,
    max: 0.0,
    default: 0.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

static PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "Maximum Amplitude",
        default: -0.1,
        ..DB
    },
    ParamSpec {
        name: "Input Boost",
        max: 50.0,
        ..DB
    },
    ParamSpec {
        name: "Look-Ahead Time",
        unit: "ms",
        min: 5.0,
        max: 20.0,
        default: 7.0,
        ..DB
    },
    ParamSpec {
        name: "Release Time",
        unit: "ms",
        min: 40.0,
        max: 200.0,
        default: 100.0,
        ..DB
    },
    ParamSpec {
        name: "Link Channels",
        unit: "",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        steps: 1,
        decimals: 0,
        ..DB
    },
];

pub static KIND: Kind = Kind {
    id: "limiter",
    name: "Limiter",
    category: "Dynamics",
    params: &PARAMS,
    make: |rate| Box::new(HardLimiter::new(rate)),
    response: None,
    transfer: Some(transfer),
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// The static curve: boosted, then held at the ceiling.
fn transfer(_rate: f64, v: &[f64], input_db: f64) -> f64 {
    (input_db + v[BOOST]).min(v[CEILING])
}

fn db_to_gain(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// Also Clip Repair's output stage, at its defaults.
pub(super) struct HardLimiter {
    rate: u32,
    /// The delay (and latency): 20 ms.
    length: usize,
    values: [f64; 5],
    boost: f32,
    ceiling: f32,
    window: usize,
    release: f32,
    linked: bool,
    /// Per channel: the boosted audio and its magnitude, `length` samples each.
    delay: [Vec<f32>; CHANNELS],
    level: [Vec<f32>; CHANNELS],
    envelope: [f32; CHANNELS],
    peak: [f32; CHANNELS],
    peak_at: [usize; CHANNELS],
    /// The slot holding the oldest sample: the one played next.
    at: usize,
    /// The number of the sample in slot `at`, counted from the start.
    played: u64,
    /// Per channel: the window's candidates for its next peak.
    peaks: [Peaks; CHANNELS],
}

/// `i` (under twice `n`) wrapped into `0..n`.
#[inline(always)]
fn wrap(i: usize, n: usize) -> usize {
    if i >= n {
        i - n
    } else {
        i
    }
}

/// The window's candidates for its peak, oldest first, their levels falling (equal levels kept,
/// so the first is the oldest of the loudest): a sliding maximum. A ring of fixed capacity,
/// allocated once.
struct Peaks {
    /// (sample number, slot, level).
    ring: Vec<(u64, usize, f32)>,
    head: usize,
    len: usize,
    /// The number of the next sample to enter.
    next: u64,
}

impl Peaks {
    fn new(capacity: usize) -> Self {
        Self {
            ring: vec![(0, 0, 0.0); capacity],
            head: 0,
            len: 0,
            next: 0,
        }
    }

    /// Empty, the next sample to enter being number `next`.
    fn clear(&mut self, next: u64) {
        self.head = 0;
        self.len = 0;
        self.next = next;
    }

    /// Let in every sample up to number `last`: `played` is the number in slot `at` of
    /// `level`'s ring (no older than the next to enter). NaN never enters, as the scan never
    /// picked one. Indices wrap by a subtraction: a division a sample is half the cost.
    fn enter(&mut self, last: u64, played: u64, at: usize, level: &[f32]) {
        let (l, cap) = (level.len(), self.ring.len());
        while self.next <= last {
            let mut slot = at + (self.next - played) as usize;
            if slot >= l {
                slot -= l;
            }
            let v = level[slot];
            if !v.is_nan() {
                while self.len > 0 && self.ring[wrap(self.head + self.len - 1, cap)].2 < v {
                    self.len -= 1;
                }
                self.ring[wrap(self.head + self.len, cap)] = (self.next, slot, v);
                self.len += 1;
            }
            self.next += 1;
        }
    }

    /// Drop the samples before number `first` (played).
    fn leave(&mut self, first: u64) {
        while self.len > 0 && self.ring[self.head].0 < first {
            self.head = wrap(self.head + 1, self.ring.len());
            self.len -= 1;
        }
    }

    /// The oldest of the loudest: its slot and level.
    fn first(&self) -> Option<(usize, f32)> {
        (self.len > 0).then(|| {
            let (_, slot, v) = self.ring[self.head];
            (slot, v)
        })
    }
}

impl HardLimiter {
    pub(super) fn new(rate: f64) -> Self {
        let rate = rate.round() as u32;
        let length = ((rate as f32) * 0.02) as usize;
        let mut limiter = Self {
            rate,
            length,
            values: [0.0; 5],
            boost: 1.0,
            ceiling: 1.0,
            window: 1,
            release: 0.0,
            linked: true,
            delay: [vec![0.0; length], vec![0.0; length]],
            level: [vec![0.0; length], vec![0.0; length]],
            envelope: [1.0; CHANNELS],
            peak: [0.0; CHANNELS],
            peak_at: [0; CHANNELS],
            at: 0,
            played: 0,
            peaks: [Peaks::new(length), Peaks::new(length)],
        };
        for (v, p) in limiter.values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        limiter.derive();
        limiter
    }

    fn derive(&mut self) {
        let v = self.values;
        self.boost = db_to_gain(v[BOOST]);
        self.ceiling = db_to_gain(v[CEILING]);
        let n = (f64::from(self.rate) * (v[LOOK_AHEAD] / 1000.0) + 0.5) as usize;
        let window = n.clamp(1, self.length - 1);
        if window != self.window {
            self.window = window;
            self.refill();
        }
        // Falls to 15 % in the release time: ln(1/0.15) over the release in samples.
        let samples = f64::from(self.rate as f32 * v[RELEASE] as f32) / 1000.0;
        self.release = (-(1.0f64 / 0.15).ln() / samples).exp() as f32;
        self.linked = v[LINK] >= 0.5;
    }

    /// The candidates afresh for the window as it now stands: when it changes size, or the
    /// lines are cleared.
    fn refill(&mut self) {
        let last = self.played + self.window as u64 - 1;
        for c in 0..CHANNELS {
            self.peaks[c].clear(self.played);
            self.peaks[c].enter(last, self.played, self.at, &self.level[c]);
        }
    }
}

impl NativeEffect for HardLimiter {
    fn set(&mut self, index: usize, value: f64) {
        if let Some(v) = self.values.get_mut(index) {
            *v = value;
            self.derive();
        }
    }

    fn latency(&self) -> usize {
        self.length
    }

    /// A fresh start when playback or an Apply starts: empty lines, the envelope at
    /// full scale. Playback and its export then start alike, whatever played before.
    fn start(&mut self) {
        for c in 0..CHANNELS {
            self.delay[c].fill(0.0);
            self.level[c].fill(0.0);
        }
        self.envelope = [1.0; CHANNELS];
        self.peak = [0.0; CHANNELS];
        self.peak_at = [0; CHANNELS];
        self.at = 0;
        self.played = 0;
        self.refill();
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let l = self.length;
        for k in from..to {
            let p = self.at;
            let mut loudest = 0.0f32;
            let last = self.played + self.window as u64 - 1;
            for c in 0..CHANNELS {
                let peaks = &mut self.peaks[c];
                peaks.leave(self.played);
                peaks.enter(last, self.played, p, &self.level[c]);
                // The window's peak: the oldest of its loudest when the last one has just
                // played, else only the sample entering the window can beat it.
                if self.peak_at[c] == (p + l - 1) % l {
                    match peaks.first() {
                        Some((at, m)) => {
                            self.peak[c] = m;
                            self.peak_at[c] = at;
                        }
                        None => self.peak[c] = -1.0,
                    }
                } else {
                    let q = (p + self.window - 1) % l;
                    if self.level[c][q] > self.peak[c] {
                        self.peak[c] = self.level[c][q];
                        self.peak_at[c] = q;
                    }
                }
                let peak = self.peak[c];
                let env = &mut self.envelope[c];
                if peak > *env {
                    // Up in a straight line, there as the peak plays.
                    let n = ((self.peak_at[c] + l - p) % l).max(1);
                    *env += (peak - *env) / n as f32;
                } else if peak <= *env * self.release {
                    *env = super::flush(*env * self.release);
                }
                loudest = loudest.max(*env);
            }
            let ceiling = self.ceiling;
            let gain_of = |env: f32| if env > ceiling { ceiling / env } else { 1.0 };
            for (c, buf) in io.iter_mut().take(CHANNELS).enumerate() {
                let gain = gain_of(if self.linked {
                    loudest
                } else {
                    self.envelope[c]
                });
                let y = self.delay[c][p] * gain;
                let x = buf[k] * self.boost;
                self.delay[c][p] = x;
                self.level[c][p] = x.abs();
                buf[k] = if y.abs() > ceiling {
                    if y > 0.0 {
                        ceiling
                    } else {
                        -ceiling
                    }
                } else {
                    y
                };
            }
            self.at = (p + 1) % l;
            self.played += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::render;
    use super::*;
    use std::collections::BTreeMap;
    use std::f64::consts::PI;

    fn tone(n: usize, level_db: f64) -> Vec<f32> {
        let a = 10f64.powf(level_db / 20.0);
        (0..n)
            .map(|i| (a * (2.0 * PI * 1000.0 * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    #[test]
    fn nothing_passes_the_ceiling_and_quiet_audio_is_untouched() {
        let settings: BTreeMap<String, f64> = [
            ("Maximum Amplitude".to_string(), -6.0),
            ("Input Boost".to_string(), 6.0),
        ]
        .into();
        let loud = tone(48_000, -1.0);
        let out = render(&KIND, 48_000.0, &settings, &[loud.clone(), loud], 512).unwrap();
        let ceiling = 10f32.powf(-6.0 / 20.0);
        assert!(out[0].iter().all(|s| s.abs() <= ceiling));
        // Settled, the tone's peaks sit at the ceiling.
        let peak = out[0][24_000..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - ceiling).abs() < 1e-3, "{peak} vs {ceiling}");
        // A quiet tone below the ceiling only gets the boost, after the 20 ms delay.
        let quiet = tone(20_000, -30.0);
        let out = render(
            &KIND,
            48_000.0,
            &settings,
            &[quiet.clone(), quiet.clone()],
            512,
        )
        .unwrap();
        let boost = db_to_gain(6.0);
        // (After the envelope, which starts at full scale, has released below it.)
        for i in 15_000..20_000 {
            assert_eq!(out[0][i], quiet[i] * boost);
        }
        assert_eq!((KIND.make)(48_000.0).latency(), 960);
    }

    #[test]
    fn the_gain_is_down_before_a_peak_plays_and_recovers_in_the_release_time() {
        // Quiet, then a full-scale step: the limiter must already be holding it down on the
        // step's first sample (look-ahead), and come back up within the release time after.
        let n = 48_000;
        let mut x = tone(n, -20.0);
        for (i, s) in x.iter_mut().enumerate().take(30_000).skip(20_000) {
            *s = (2.0 * PI * 1000.0 * i as f64 / 48_000.0).sin() as f32;
        }
        let settings: BTreeMap<String, f64> = [("Maximum Amplitude".to_string(), -12.0)].into();
        let out = render(&KIND, 48_000.0, &settings, &[x.clone(), x], 64).unwrap();
        let ceiling = 10f32.powf(-12.0 / 20.0);
        let peak = |r: std::ops::Range<usize>| out[0][r].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak(20_000..20_200) <= ceiling * 1.0001);
        // After the step, the quiet tone (-20 dBFS, under the ceiling) is back within 1 dB
        // 200 ms on: the envelope falls to 15 % every 100 ms.
        let quiet = 10f32.powf(-20.0 / 20.0);
        let back = peak(39_600..40_000);
        assert!((20.0 * (back / quiet).log10()).abs() < 1.0, "{back}");
    }

    #[test]
    fn linked_channels_share_the_louder_ones_gain() {
        let loud = tone(24_000, 0.0);
        let quiet = tone(24_000, -20.0);
        let settings: BTreeMap<String, f64> = [("Maximum Amplitude".to_string(), -6.0)].into();
        let out = render(
            &KIND,
            48_000.0,
            &settings,
            &[loud.clone(), quiet.clone()],
            512,
        )
        .unwrap();
        // The quiet right channel is turned down by the loud left's 6 dB too.
        let ratio = out[1][20_000] / quiet[20_000];
        assert!((20.0 * ratio.log10() + 6.0).abs() < 0.2, "{ratio}");
        let mut unlinked = settings.clone();
        unlinked.insert("Link Channels".into(), 0.0);
        let out = render(&KIND, 48_000.0, &unlinked, &[loud, quiet.clone()], 512).unwrap();
        assert_eq!(out[1][20_000], quiet[20_000]);
    }

    /// The oldest of the loudest samples in `limiter`'s window for channel `c`, by a scan of
    /// the whole window: what the limiter used to do whenever its peak had played.
    fn scanned(limiter: &HardLimiter, c: usize) -> Option<(usize, f32)> {
        let l = limiter.length;
        let mut best: Option<(usize, f32)> = None;
        for i in 0..limiter.window {
            let q = (limiter.at + i) % l;
            let v = limiter.level[c][q];
            if v > best.map_or(-1.0, |b| b.1) {
                best = Some((q, v));
            }
        }
        best
    }

    #[test]
    fn the_queue_finds_the_peak_a_scan_of_the_window_finds() {
        // Silence, held levels (ties everywhere), noise, a rising ramp that never lets its peak
        // play, a NaN, and the look-ahead changed mid-way both ways: whenever the peak has
        // just played, the queue's pick is the scan's, slot and level.
        let mut limiter = HardLimiter::new(48_000.0);
        let mut seed = 1u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let mut x: Vec<f32> = Vec::new();
        x.extend(std::iter::repeat_n(0.0, 3000));
        x.extend(std::iter::repeat_n(0.25, 2000));
        x.extend((0..2000).map(|i| if i % 7 < 3 { 0.5 } else { -0.5 }));
        x.extend((0..6000).map(|_| noise()));
        x.extend((0..3000).map(|i| i as f32 / 3000.0));
        x.push(f32::NAN);
        x.extend((0..4000).map(|_| noise() * 0.1));
        x.extend(std::iter::repeat_n(0.0, 3000));
        let mut checked = 0;
        for (k, &v) in x.iter().enumerate() {
            match k {
                6000 => limiter.set(LOOK_AHEAD, 20.0),
                11_000 => limiter.set(LOOK_AHEAD, 5.0),
                19_000 => limiter.set(LOOK_AHEAD, 12.0),
                _ => {}
            }
            let p = limiter.at;
            let l = limiter.length;
            let rescans: Vec<bool> = (0..CHANNELS)
                .map(|c| limiter.peak_at[c] == (p + l - 1) % l)
                .collect();
            let expected: Vec<_> = (0..CHANNELS).map(|c| scanned(&limiter, c)).collect();
            let mut io = vec![vec![v], vec![v * 0.5]];
            limiter.process(&mut io, 0, 1);
            for c in 0..CHANNELS {
                if rescans[c] {
                    checked += 1;
                    match expected[c] {
                        Some((at, m)) => {
                            assert_eq!(limiter.peak_at[c], at, "sample {k}, channel {c}");
                            assert_eq!(limiter.peak[c].to_bits(), m.to_bits(), "sample {k}");
                        }
                        None => assert_eq!(limiter.peak[c], -1.0, "sample {k}"),
                    }
                }
            }
        }
        // Silence and held levels play their peak every sample.
        assert!(checked > 10_000, "{checked}");
    }

    #[test]
    fn silence_costs_no_scan_and_the_envelope_reaches_zero() {
        // A burst, then 5 s of silence: the envelope has fallen under the flush floor to an
        // exact zero rather than through the denormals, and the window's queue holds the
        // window, never more.
        let mut limiter = HardLimiter::new(48_000.0);
        let burst = tone(4800, 0.0);
        let mut io = vec![burst.clone(), burst];
        limiter.process(&mut io, 0, 4800);
        let mut silence = vec![vec![0.0f32; 48_000]; CHANNELS];
        for _ in 0..5 {
            silence.iter_mut().for_each(|c| c.fill(0.0));
            limiter.process(&mut silence, 0, 48_000);
        }
        assert_eq!(limiter.envelope, [0.0; CHANNELS]);
        for (peaks, out) in limiter.peaks.iter().zip(&silence) {
            assert!(peaks.len <= limiter.window);
            assert!(out.iter().all(|s| *s == 0.0));
        }
    }
}
