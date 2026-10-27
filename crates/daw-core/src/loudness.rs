//! Loudness metering to EBU R 128 ("EBU Mode": EBU Tech 3341 and 3342, ITU-R BS.1770-4):
//! momentary (400 ms), short-term (3 s), gated integrated loudness, loudness range (LRA) and
//! true peak (4× oversampled), with their maxima.
//!
//! - **K-weighting** is two biquads derived from their analog prototypes, so it is right at any
//!   sample rate; at 48 kHz it matches BS.1770's table (tested).
//! - **Time:** audio is summed into 10 ms sub-blocks. Momentary is the last 40, short-term the
//!   last 300, both updated every 10 ms: fine enough that a 400 ms tone at any offset reaches
//!   its full level in the maximum (Tech 3341 cases 13 and 14). Gating blocks (400 ms, 75 %
//!   overlap) and LRA's short-term samples are taken every 100 ms.
//! - **Gating without lists:** gating blocks and short-term values go into histograms of
//!   0.01 LU bins that keep, per bin, a count and the exact energy sum. Gated means are then
//!   exact and only the gate decision is quantised, and nothing grows: the meter allocates
//!   when made, never while metering, so it can run on the audio thread.
//! - **Running:** momentary and short-term are always measured. Integrated, LRA, true peak
//!   and the maxima accumulate only while the meter runs, and only [`LoudnessMeter::reset`]
//!   clears them, as Tech 3341 §2.2 asks of a live meter.

use serde::Serialize;
use std::f64::consts::PI;

/// The gates, from BS.1770 and Tech 3342.
const ABSOLUTE_GATE: f64 = -70.0;
const RELATIVE_GATE: f64 = -10.0;
const LRA_RELATIVE_GATE: f64 = -20.0;

/// Sub-blocks per second, and how many make each window.
const SUB_BLOCKS_PER_SECOND: usize = 100;
const MOMENTARY: usize = 40;
const SHORT_TERM: usize = 300;
/// Gating blocks and LRA samples every 100 ms.
const GATE_EVERY: usize = 10;

/// The histograms cover −70 to +10 LUFS in 0.01 LU bins; louder values land in the top bin
/// (their energy is still exact).
const HISTOGRAM_FLOOR: f64 = ABSOLUTE_GATE;
const HISTOGRAM_STEP: f64 = 0.01;
const HISTOGRAM_BINS: usize = 8000;

/// One second-order section (transposed direct form II, in f64).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    pub b: [f64; 3],
    /// a1 and a2 (a0 is 1).
    pub a: [f64; 2],
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self {
            b,
            a,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z1;
        self.z1 = self.b[1] * x - self.a[0] * y + self.z2;
        self.z2 = self.b[2] * x - self.a[1] * y;
        y
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// BS.1770's K-weighting at `rate`: a high shelf (about +4 dB above 1.5 kHz, the head's
/// effect) then the RLB high-pass. Derived from the analog prototypes rather than taken from
/// BS.1770's 48 kHz table, so 44.1 kHz and the rest are right too.
pub fn k_weighting(rate: f64) -> [Biquad; 2] {
    let (f0, gain_db, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (PI * f0 / rate).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad::new(
        [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad::new(
        [1.0, -2.0, 1.0],
        [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    [shelf, high_pass]
}

/// Taps per phase of the true-peak interpolator: a 97-tap Kaiser-windowed sinc at 4× the rate,
/// split into four phases (padded to 25). Phase 0 passes the samples themselves through, the
/// other three fall between them.
const PEAK_TAPS: usize = 25;
const PEAK_PHASES: usize = 4;

/// The interpolator's taps, phase by phase.
fn peak_taps() -> [[f64; PEAK_TAPS]; PEAK_PHASES] {
    let n = PEAK_TAPS * PEAK_PHASES - 3; // 97: odd, so the centre falls on a sample
    let centre = (n - 1) as f64 / 2.0;
    let cutoff = 0.5 / PEAK_PHASES as f64; // the original Nyquist, in cycles per 4× sample
    let beta = 8.0;
    let mut taps = [[0.0; PEAK_TAPS]; PEAK_PHASES];
    for i in 0..n {
        let x = i as f64 - centre;
        let sinc = if x == 0.0 {
            1.0
        } else {
            (2.0 * PI * cutoff * x).sin() / (2.0 * PI * cutoff * x)
        };
        let r = x / centre;
        let window = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta);
        // Gain 4 overall, so each phase passes a signal at about unity.
        taps[i % PEAK_PHASES][i / PEAK_PHASES] = 2.0 * cutoff * PEAK_PHASES as f64 * sinc * window;
    }
    taps
}

/// The modified Bessel function of the first kind, order 0, for the Kaiser window.
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > sum * 1e-12 {
        term *= (x / (2.0 * k)).powi(2);
        sum += term;
        k += 1.0;
    }
    sum
}

/// One channel's true-peak detector. The history is kept twice over, so the taps always read
/// one contiguous slice of it, newest first.
#[derive(Debug, Clone)]
struct TruePeak {
    history: [f64; 2 * PEAK_TAPS],
    at: usize,
    peak: f64,
}

impl TruePeak {
    fn new() -> Self {
        Self {
            history: [0.0; 2 * PEAK_TAPS],
            at: 0,
            peak: 0.0,
        }
    }

    #[inline]
    fn run(&mut self, taps: &[[f64; PEAK_TAPS]; PEAK_PHASES], x: f64) {
        self.at = if self.at == 0 {
            PEAK_TAPS - 1
        } else {
            self.at - 1
        };
        self.history[self.at] = x;
        self.history[self.at + PEAK_TAPS] = x;
        let recent = &self.history[self.at..self.at + PEAK_TAPS];
        for phase in taps {
            let y: f64 = phase.iter().zip(recent).map(|(t, h)| t * h).sum();
            self.peak = self.peak.max(y.abs());
        }
    }
}
/// Loudness levels in fixed bins, each with a count and the exact sum of its energies.
#[derive(Debug, Clone)]
struct Histogram {
    count: Box<[u64]>,
    energy: Box<[f64]>,
}

impl Histogram {
    fn new() -> Self {
        Self {
            count: vec![0; HISTOGRAM_BINS].into_boxed_slice(),
            energy: vec![0.0; HISTOGRAM_BINS].into_boxed_slice(),
        }
    }

    fn clear(&mut self) {
        self.count.fill(0);
        self.energy.fill(0.0);
    }

    fn bin(level: f64) -> usize {
        (((level - HISTOGRAM_FLOOR) / HISTOGRAM_STEP)
            .floor()
            .max(0.0) as usize)
            .min(HISTOGRAM_BINS - 1)
    }

    fn centre(bin: usize) -> f64 {
        HISTOGRAM_FLOOR + (bin as f64 + 0.5) * HISTOGRAM_STEP
    }

    fn add(&mut self, level: f64, energy: f64) {
        let bin = Self::bin(level);
        self.count[bin] += 1;
        self.energy[bin] += energy;
    }

    /// Count and energy of the bins from `first` up.
    fn sum_from(&self, first: usize) -> (u64, f64) {
        (first..HISTOGRAM_BINS).fold((0, 0.0), |(n, e), b| {
            (n + self.count[b], e + self.energy[b])
        })
    }

    /// The first bin whose centre is at or above `level`: the bins a gate there lets through.
    fn first_at_or_above(level: f64) -> usize {
        (0..HISTOGRAM_BINS)
            .find(|&b| Self::centre(b) >= level)
            .unwrap_or(HISTOGRAM_BINS)
    }

    /// The level of the `k`th value (0-based, ascending) among the bins from `first` up.
    fn nth_from(&self, first: usize, k: u64) -> f64 {
        let mut seen = 0;
        for b in first..HISTOGRAM_BINS {
            seen += self.count[b];
            if seen > k {
                return Self::centre(b);
            }
        }
        Self::centre(HISTOGRAM_BINS - 1)
    }
}

/// Readings never go below this: digital silence reads it rather than minus infinity, which
/// JSON can't carry and which would look like "nothing measured".
pub const SILENCE: f64 = -120.0;

/// Mean-square energy to loudness, in LUFS (BS.1770 eq. 2, channel weights 1 for L and R).
fn lufs(energy: f64) -> f64 {
    -0.691 + 10.0 * energy.log10()
}

/// A reading, floored at [`SILENCE`].
fn reading(level: f64) -> f64 {
    if level.is_finite() {
        level.max(SILENCE)
    } else {
        SILENCE
    }
}

/// What the meter reads. Levels are LUFS, LRA LU, true peak dBTP; `None` until there is
/// enough to measure (400 ms for momentary, 3 s for short-term, one gating block above the
/// −70 LUFS gate for integrated, any audio for true peak). Silence reads [`SILENCE`].
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, schemars::JsonSchema)]
pub struct Loudness {
    pub momentary: Option<f64>,
    pub short_term: Option<f64>,
    pub integrated: Option<f64>,
    /// Loudness range (EBU Tech 3342). Not yet stable in the first 60 s (Tech 3341 §2.4).
    pub range: Option<f64>,
    pub true_peak: Option<f64>,
    pub max_momentary: Option<f64>,
    pub max_short_term: Option<f64>,
    /// How long the integrated measurement has run, in seconds.
    pub seconds: f64,
}

/// An EBU Mode loudness meter for one stereo (or mono) signal. See the module docs.
#[derive(Debug, Clone)]
pub struct LoudnessMeter {
    rate: u32,
    sub_block: usize,
    filters: [[Biquad; 2]; 2],
    peak_taps: [[f64; PEAK_TAPS]; PEAK_PHASES],
    peaks: [TruePeak; 2],
    /// The sub-block being summed: K-weighted energy over both channels, and its length.
    sum: f64,
    summed: usize,
    /// The last [`SHORT_TERM`] sub-blocks' energies, newest at `head - 1`.
    ring: Box<[f64]>,
    head: usize,
    /// Sub-blocks in the ring since the last reset (at most its length).
    filled: usize,
    running: bool,
    /// Sub-blocks measured while running since the last reset.
    run_blocks: u64,
    /// Sub-blocks measured since the meter last started running: gating blocks and maxima take
    /// only windows wholly inside it, so audio from a pause never counts.
    streak: u64,
    gates: Histogram,
    short_terms: Histogram,
    readings: Loudness,
}

impl LoudnessMeter {
    /// A meter for `rate` Hz, running. Allocates (its histograms); nothing after this does.
    pub fn new(rate: u32) -> Self {
        let mut meter = Self {
            rate,
            sub_block: 1,
            filters: [k_weighting(f64::from(rate)); 2],
            peak_taps: peak_taps(),
            peaks: [TruePeak::new(), TruePeak::new()],
            sum: 0.0,
            summed: 0,
            ring: vec![0.0; SHORT_TERM].into_boxed_slice(),
            head: 0,
            filled: 0,
            running: true,
            run_blocks: 0,
            streak: 0,
            gates: Histogram::new(),
            short_terms: Histogram::new(),
            readings: Loudness::default(),
        };
        meter.set_rate(rate);
        meter
    }

    /// Re-derive the filters for a new sample rate and reset. Allocation-free.
    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate;
        self.sub_block = (rate as usize / SUB_BLOCKS_PER_SECOND).max(1);
        self.filters = [k_weighting(f64::from(rate)); 2];
        self.reset();
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Start or pause the integrated measurement (integrated, LRA, true peak, maxima).
    pub fn set_running(&mut self, running: bool) {
        if running && !self.running {
            self.streak = 0;
        }
        self.running = running;
    }

    /// Clear everything, as before a new measurement. Allocation-free.
    pub fn reset(&mut self) {
        for channel in &mut self.filters {
            channel.iter_mut().for_each(Biquad::reset);
        }
        self.peaks = [TruePeak::new(), TruePeak::new()];
        self.sum = 0.0;
        self.summed = 0;
        self.ring.fill(0.0);
        self.head = 0;
        self.filled = 0;
        self.run_blocks = 0;
        self.streak = 0;
        self.gates.clear();
        self.short_terms.clear();
        self.readings = Loudness::default();
    }

    pub fn readings(&self) -> Loudness {
        self.readings
    }

    /// Meter a block: `right` is `None` for a mono signal (one channel, weight 1).
    /// Allocation-free.
    pub fn process(&mut self, left: &[f32], right: Option<&[f32]>) {
        for (i, &l) in left.iter().enumerate() {
            let l = f64::from(l);
            let kl = self.filters[0].iter_mut().fold(l, |x, stage| stage.run(x));
            let mut energy = kl * kl;
            if self.running {
                self.peaks[0].run(&self.peak_taps, l);
            }
            if let Some(&r) = right.and_then(|r| r.get(i)) {
                let r = f64::from(r);
                let kr = self.filters[1].iter_mut().fold(r, |x, stage| stage.run(x));
                energy += kr * kr;
                if self.running {
                    self.peaks[1].run(&self.peak_taps, r);
                }
            }
            self.sum += energy;
            self.summed += 1;
            if self.summed == self.sub_block {
                self.end_sub_block();
            }
        }
    }

    /// The mean-square energy of the last `n` sub-blocks.
    fn window(&self, n: usize) -> f64 {
        let total: f64 = (1..=n)
            .map(|k| self.ring[(self.head + SHORT_TERM - k) % SHORT_TERM])
            .sum();
        total / (n * self.sub_block) as f64
    }

    fn end_sub_block(&mut self) {
        self.ring[self.head] = self.sum;
        self.head = (self.head + 1) % SHORT_TERM;
        self.sum = 0.0;
        self.summed = 0;
        self.filled = (self.filled + 1).min(SHORT_TERM);
        let momentary = (self.filled >= MOMENTARY).then(|| self.window(MOMENTARY));
        let short_term = (self.filled >= SHORT_TERM).then(|| self.window(SHORT_TERM));
        self.readings.momentary = momentary.map(|e| reading(lufs(e)));
        self.readings.short_term = short_term.map(|e| reading(lufs(e)));
        if !self.running {
            return;
        }
        self.run_blocks += 1;
        self.streak += 1;
        // Maxima and gating only over windows wholly inside the measurement.
        let whole = |n: usize| self.streak >= n as u64;
        let max = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        if whole(MOMENTARY) {
            self.readings.max_momentary = max(self.readings.max_momentary, self.readings.momentary);
        }
        if whole(SHORT_TERM) {
            self.readings.max_short_term =
                max(self.readings.max_short_term, self.readings.short_term);
        }
        if self.streak.is_multiple_of(GATE_EVERY as u64) {
            if let (true, Some(energy)) = (whole(MOMENTARY), momentary) {
                let level = lufs(energy);
                if level > ABSOLUTE_GATE {
                    self.gates.add(level, energy);
                }
            }
            if let (true, Some(level)) = (whole(SHORT_TERM), self.readings.short_term) {
                if level >= ABSOLUTE_GATE {
                    self.short_terms.add(level, 10f64.powf(level / 10.0));
                }
            }
            self.readings.integrated = self.integrated();
            self.readings.range = self.range();
        }
        self.readings.seconds = self.run_blocks as f64 / SUB_BLOCKS_PER_SECOND as f64;
        let peak = self.peaks[0].peak.max(self.peaks[1].peak);
        self.readings.true_peak = Some(reading(20.0 * peak.log10()));
    }

    /// BS.1770's gated loudness: the gating blocks above −70 LUFS, then those above 10 LU
    /// below their mean energy.
    fn integrated(&self) -> Option<f64> {
        let (n, energy) = self.gates.sum_from(0);
        if n == 0 {
            return None;
        }
        let gate = lufs(energy / n as f64) + RELATIVE_GATE;
        let (n, energy) = self.gates.sum_from(Histogram::first_at_or_above(gate));
        (n > 0).then(|| lufs(energy / n as f64))
    }

    /// Tech 3342's loudness range: the short-term levels above −70 LUFS, then those at or
    /// above 20 LU below their mean power; the spread between their 10th and 95th
    /// percentiles, picked as its MATLAB reference does.
    fn range(&self) -> Option<f64> {
        let (n, power) = self.short_terms.sum_from(0);
        if n == 0 {
            return None;
        }
        let gate = 10.0 * (power / n as f64).log10() + LRA_RELATIVE_GATE;
        let first = Histogram::first_at_or_above(gate);
        let (n, _) = self.short_terms.sum_from(first);
        if n == 0 {
            return None;
        }
        let at = |percent: f64| ((n - 1) as f64 * percent / 100.0).round() as u64;
        Some(
            self.short_terms.nth_from(first, at(95.0)) - self.short_terms.nth_from(first, at(10.0)),
        )
    }
}

/// Measure a whole signal (`right` `None` for mono) at `rate`: what a file-based meter reads.
pub fn measure(left: &[f32], right: Option<&[f32]>, rate: u32) -> Loudness {
    let mut meter = LoudnessMeter::new(rate);
    meter.process(left, right);
    meter.readings()
}

/// The gain that normalising applies (see [`normalise_gain`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Gain {
    /// In dB: positive is louder.
    pub db: f64,
    /// The true-peak ceiling held the gain below what the target needed.
    pub held_by_peak: bool,
}

/// The constant gain that takes material at `integrated` loudness to `target` (LUFS), held
/// back so its true peak stays at or under `ceiling` (dBTP). `None` when there is nothing to
/// normalise: no gating block was above −70 LUFS (silence would only be made louder silence).
///
/// One gain for the whole file, with no limiting: normalising never changes the sound beyond
/// its level, so a single transient can hold the gain back. A float file can't clip, so the
/// ceiling is the loudness standard's limit (R 128 and the streaming services' −1 dBTP), not
/// a safety margin.
pub fn normalise_gain(
    integrated: Option<f64>,
    true_peak: Option<f64>,
    target: f64,
    ceiling: f64,
) -> Option<Gain> {
    let wanted = target - integrated?;
    match true_peak {
        Some(peak) if peak + wanted > ceiling => Some(Gain {
            db: ceiling - peak,
            held_by_peak: true,
        }),
        _ => Some(Gain {
            db: wanted,
            held_by_peak: false,
        }),
    }
}

/// Measure a WAV file: stereo as left and right, mono as one channel at weight 1 (about 3 dB
/// quieter than the same audio panned centre in a stereo mix). Files with more channels are
/// refused: BS.1770 weights surround channels differently, and this meter is stereo.
pub fn measure_file(path: &std::path::Path) -> anyhow::Result<Loudness> {
    let audio = crate::media::read_audio(path)?;
    match audio.channels.as_slice() {
        [mono] => Ok(measure(mono, None, audio.sample_rate)),
        [left, right] => Ok(measure(left, Some(right), audio.sample_rate)),
        channels => anyhow::bail!(
            "{} has {} channels; the loudness meter measures mono and stereo",
            path.display(),
            channels.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// A 1 kHz sine with peak level `db` dBFS for `seconds`, as Tech 3341's tones are.
    fn tone(db: f64, seconds: f64, rate: u32) -> Vec<f32> {
        let amplitude = 10f64.powf(db / 20.0);
        let n = (seconds * f64::from(rate)).round() as usize;
        (0..n)
            .map(|i| (amplitude * (2.0 * PI * 1000.0 * i as f64 / f64::from(rate)).sin()) as f32)
            .collect()
    }

    fn silence(seconds: f64, rate: u32) -> Vec<f32> {
        vec![0.0; (seconds * f64::from(rate)).round() as usize]
    }

    /// Tones and silences one after another (silence where the level is `None`).
    fn sequence(parts: &[(Option<f64>, f64)], rate: u32) -> Vec<f32> {
        parts
            .iter()
            .flat_map(|&(db, seconds)| match db {
                Some(db) => tone(db, seconds, rate),
                None => silence(seconds, rate),
            })
            .collect()
    }

    /// Stereo, the same signal in both channels (Tech 3341: "applied in phase to both").
    fn stereo(signal: &[f32], rate: u32) -> Loudness {
        measure(signal, Some(signal), rate)
    }

    fn near(value: Option<f64>, expected: f64, tolerance: f64) -> bool {
        value.is_some_and(|v| (v - expected).abs() <= tolerance + 1e-9)
    }

    #[test]
    fn k_weighting_matches_the_bs1770_table_at_48k() {
        let [shelf, high_pass] = k_weighting(48_000.0);
        let table_shelf = (
            [1.53512485958697, -2.69169618940638, 1.19839281085285],
            [-1.69065929318241, 0.73248077421585],
        );
        let table_high_pass = ([1.0, -2.0, 1.0], [-1.99004745483398, 0.99007225036621]);
        for (got, want) in [(shelf, table_shelf), (high_pass, table_high_pass)] {
            for (g, w) in got.b.iter().zip(want.0).chain(got.a.iter().zip(want.1)) {
                assert!((g - w).abs() < 1e-6, "{g} vs {w}");
            }
        }
    }

    /// Cases 1 and 2, and 1 again at 44.1 kHz: M, S and I of a steady tone.
    #[test]
    fn steady_tones_read_their_level() {
        for (db, rate) in [(-23.0, RATE), (-33.0, RATE), (-23.0, 44_100)] {
            let l = stereo(&tone(db, 20.0, rate), rate);
            for (name, value) in [("M", l.momentary), ("S", l.short_term), ("I", l.integrated)] {
                assert!(
                    near(value, db, 0.1),
                    "case {db} dBFS at {rate}: {name} = {value:?}"
                );
            }
        }
    }

    /// Cases 3, 4 and 5: the gates leave -23 LUFS.
    #[test]
    fn gating_finds_the_programme_level() {
        let cases: [&[(Option<f64>, f64)]; 3] = [
            &[
                (Some(-36.0), 10.0),
                (Some(-23.0), 60.0),
                (Some(-36.0), 10.0),
            ],
            &[
                (Some(-72.0), 10.0),
                (Some(-36.0), 10.0),
                (Some(-23.0), 60.0),
                (Some(-36.0), 10.0),
                (Some(-72.0), 10.0),
            ],
            &[
                (Some(-26.0), 20.0),
                (Some(-20.0), 20.1),
                (Some(-26.0), 20.0),
            ],
        ];
        for (k, parts) in cases.iter().enumerate() {
            let l = stereo(&sequence(parts, RATE), RATE);
            assert!(
                near(l.integrated, -23.0, 0.1),
                "case {}: I = {:?}",
                k + 3,
                l.integrated
            );
        }
    }

    /// Case 9: S is constant at -23 after 3 s; case 12: M is constant at -23 after 1 s.
    #[test]
    fn windows_are_rectangular_and_exactly_long_enough() {
        for (case, high, low, seconds, settle, short) in [
            (9, 1.34, 1.66, 15.0, 3.0, true),
            (12, 0.18, 0.22, 10.0, 1.0, false),
        ] {
            let period: Vec<(Option<f64>, f64)> = vec![(Some(-20.0), high), (Some(-30.0), low)];
            let repeats = (seconds / (high + low)).round() as usize;
            let parts: Vec<_> = period.iter().cycle().take(2 * repeats).copied().collect();
            let signal = sequence(&parts, RATE);
            let mut meter = LoudnessMeter::new(RATE);
            let step = (RATE / 100) as usize;
            for (k, chunk) in signal.chunks(step).enumerate() {
                meter.process(chunk, Some(chunk));
                let t = (k + 1) as f64 / 100.0;
                if t >= settle {
                    let l = meter.readings();
                    let value = if short { l.short_term } else { l.momentary };
                    assert!(
                        near(value, -23.0, 0.1),
                        "case {case} at {t:.2} s: {value:?}"
                    );
                }
            }
        }
    }

    /// Cases 10 and 13 (file-based, each segment measured alone): the maximum reaches -23
    /// wherever the tone starts.
    #[test]
    fn maxima_catch_a_tone_at_any_offset() {
        for i in 0..20 {
            let s = stereo(
                &sequence(
                    &[(None, i as f64 * 0.15), (Some(-23.0), 3.0), (None, 1.0)],
                    RATE,
                ),
                RATE,
            );
            assert!(
                near(s.max_short_term, -23.0, 0.1),
                "case 10, segment {i}: {:?}",
                s.max_short_term
            );
            let m = stereo(
                &sequence(
                    &[(None, i as f64 * 0.02), (Some(-23.0), 0.4), (None, 1.0)],
                    RATE,
                ),
                RATE,
            );
            assert!(
                near(m.max_momentary, -23.0, 0.1),
                "case 13, segment {i}: {:?}",
                m.max_momentary
            );
        }
    }

    /// Cases 11 and 14 (live): one long signal, the maximum read after each tone climbs a dB
    /// at a time.
    #[test]
    fn live_maxima_climb_with_each_tone() {
        for (case, tone_s, step_s) in [(11, 3.0, 0.15), (14, 0.4, 0.02)] {
            let mut meter = LoudnessMeter::new(RATE);
            for i in 0..20 {
                let level = -38.0 + i as f64;
                let segment = sequence(
                    &[
                        (None, i as f64 * step_s),
                        (Some(level), tone_s),
                        (None, tone_s - i as f64 * step_s),
                    ],
                    RATE,
                );
                meter.process(&segment, Some(&segment));
                let l = meter.readings();
                let max = if case == 11 {
                    l.max_short_term
                } else {
                    l.max_momentary
                };
                assert!(
                    near(max, level, 0.1),
                    "case {case}, tone {i}: {max:?} vs {level}"
                );
            }
        }
    }

    /// Tech 3342 cases 1 to 4.
    #[test]
    fn loudness_range_spans_the_tones() {
        let cases: [(&[f64], f64); 4] = [
            (&[-20.0, -30.0], 10.0),
            (&[-20.0, -15.0], 5.0),
            (&[-40.0, -20.0], 20.0),
            (&[-50.0, -35.0, -20.0, -35.0, -50.0], 15.0),
        ];
        for (k, (levels, lra)) in cases.iter().enumerate() {
            let parts: Vec<_> = levels.iter().map(|&db| (Some(db), 20.0)).collect();
            let l = stereo(&sequence(&parts, RATE), RATE);
            assert!(
                near(l.range, *lra, 1.0),
                "3342 case {}: LRA = {:?}",
                k + 1,
                l.range
            );
        }
    }

    /// A sine at `freq` with peak `amplitude` and starting `phase` (degrees), tapered in and out
    /// over 10 ms as Tech 3341 asks.
    fn peak_sine(freq: f64, amplitude: f64, phase: f64, rate: u32) -> Vec<f32> {
        let n = rate as usize;
        let fade = rate as usize / 100;
        (0..n)
            .map(|i| {
                let taper = (i.min(n - 1 - i) as f64 / fade as f64).min(1.0);
                let x = 2.0 * PI * freq * i as f64 / f64::from(rate) + phase.to_radians();
                (taper * amplitude * x.sin()) as f32
            })
            .collect()
    }

    /// Cases 15 to 19: +0.2/-0.4 dB of the true peak, where samples miss it by up to 3 dB.
    #[test]
    fn true_peak_finds_the_peak_between_samples() {
        let fs = f64::from(RATE);
        for (case, freq, amplitude, phase, expected) in [
            (15, fs / 4.0, 0.5, 0.0, -6.0),
            (16, fs / 4.0, 0.5, 45.0, -6.0),
            (17, fs / 6.0, 0.5, 60.0, -6.0),
            (18, fs / 8.0, 0.5, 67.5, -6.0),
            (19, fs / 4.0, 1.41, 45.0, 3.0),
        ] {
            let signal = peak_sine(freq, amplitude, phase, RATE);
            let tp = stereo(&signal, RATE).true_peak.unwrap();
            assert!(
                tp <= expected + 0.2 && tp >= expected - 0.4,
                "case {case}: {tp:.2} dBTP, expected {expected}"
            );
        }
    }

    /// Cases 20 to 23: a single period of fs/4 at full scale inside an fs/6 sine, made at 4×
    /// the rate, filtered and taken down to the rate at each of the four offsets.
    #[test]
    fn true_peak_of_a_burst_at_every_offset() {
        let fast = 4 * RATE as usize;
        let n = fast / 10; // 0.1 s at 4x: enough around the burst
        let burst_at = n / 2 - (n / 2) % 24; // where the fs/6 sine crosses zero rising
        let burst = 16; // one period of fs/4 at 4x
        let signal: Vec<f64> = (0..n)
            .map(|i| {
                let taper = (i.min(n - 1 - i) as f64 / (fast as f64 / 500.0)).min(1.0);
                // The fs/6 sine stops for the burst and picks up where it left off.
                let v = if (burst_at..burst_at + burst).contains(&i) {
                    (2.0 * PI * (i - burst_at) as f64 / burst as f64).sin()
                } else {
                    let j = if i < burst_at { i } else { i - burst };
                    0.5 * (2.0 * PI * j as f64 / 24.0).sin()
                };
                taper * v
            })
            .collect();
        // Anti-alias at 4x: a long Kaiser-windowed sinc just under the target rate's Nyquist.
        let taps = 511;
        let centre = (taps - 1) as f64 / 2.0;
        let cutoff = 0.5 / 4.0 * 0.95;
        let filter: Vec<f64> = (0..taps)
            .map(|i| {
                let x = i as f64 - centre;
                let sinc = if x == 0.0 {
                    2.0 * cutoff
                } else {
                    (2.0 * PI * cutoff * x).sin() / (PI * x)
                };
                let r = x / centre;
                sinc * bessel_i0(10.0 * (1.0 - r * r).sqrt()) / bessel_i0(10.0)
            })
            .collect();
        let filtered: Vec<f64> = (0..n)
            .map(|i| {
                filter
                    .iter()
                    .enumerate()
                    .filter_map(|(k, t)| {
                        (i + k)
                            .checked_sub(taps / 2)
                            .and_then(|j| signal.get(j))
                            .map(|s| t * s)
                    })
                    .sum()
            })
            .collect();
        for offset in 0..4 {
            let down: Vec<f32> = filtered
                .iter()
                .skip(offset)
                .step_by(4)
                .map(|&v| v as f32)
                .collect();
            let tp = stereo(&down, RATE).true_peak.unwrap();
            assert!(
                (-0.4..=0.2).contains(&tp),
                "case {}: {tp:.2} dBTP, expected 0.0",
                20 + offset
            );
        }
    }

    #[test]
    fn a_paused_meter_keeps_its_integrated_reading_and_reset_clears_it() {
        let mut meter = LoudnessMeter::new(RATE);
        let loud = tone(-23.0, 5.0, RATE);
        meter.process(&loud, Some(&loud));
        let before = meter.readings();
        meter.set_running(false);
        let quiet = tone(-40.0, 5.0, RATE);
        meter.process(&quiet, Some(&quiet));
        let paused = meter.readings();
        assert_eq!(
            paused.integrated, before.integrated,
            "integrated holds while paused"
        );
        assert_eq!(paused.seconds, before.seconds);
        assert!(
            near(paused.momentary, -40.0, 0.1),
            "momentary always measures"
        );
        meter.reset();
        assert_eq!(meter.readings(), Loudness::default());

        // A second of -23, a pause over -40, then 400 ms of -23 again: the gating blocks after
        // the pause start with it, so none mixes in the quiet audio heard while paused.
        let mut meter = LoudnessMeter::new(RATE);
        let second = tone(-23.0, 1.0, RATE);
        meter.process(&second, Some(&second));
        meter.set_running(false);
        meter.process(&quiet, Some(&quiet));
        meter.set_running(true);
        let short = tone(-23.0, 0.4, RATE);
        meter.process(&short, Some(&short));
        let l = meter.readings();
        assert!(
            near(l.integrated, -23.0, 0.05),
            "after a pause: {:?}",
            l.integrated
        );
        assert!((l.seconds - 1.4).abs() < 1e-9);
    }

    #[test]
    fn normalising_reaches_the_target_unless_the_peak_holds_it() {
        let gain = |i, tp| normalise_gain(i, tp, -14.0, -1.0);
        assert_eq!(
            gain(Some(-23.0), Some(-20.0)),
            Some(Gain {
                db: 9.0,
                held_by_peak: false
            })
        );
        assert_eq!(
            gain(Some(-8.0), Some(-0.5)),
            Some(Gain {
                db: -6.0,
                held_by_peak: false
            }),
            "too loud: turned down"
        );
        assert_eq!(
            gain(Some(-23.0), Some(-2.0)),
            Some(Gain {
                db: 1.0,
                held_by_peak: true
            }),
            "+9 dB would put the peak at +7; the ceiling allows +1"
        );
        assert_eq!(gain(None, Some(SILENCE)), None, "nothing to normalise");
    }

    #[test]
    fn silence_reads_the_floor_not_minus_infinity() {
        let l = stereo(&silence(1.0, RATE), RATE);
        assert_eq!(l.momentary, Some(SILENCE));
        assert_eq!(l.true_peak, Some(SILENCE));
        assert_eq!(l.integrated, None, "nothing above the -70 LUFS gate");
    }
}
