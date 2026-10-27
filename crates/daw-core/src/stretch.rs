//! Time-stretch and pitch-shift (dev plan 7.1): Signalsmith Stretch 1.3.1 (MIT), through the
//! `signalsmith-stretch` crate.
//!
//! The goal is élastique-style editing: a DI (or any audio) made longer or shorter without
//! its pitch moving, or transposed without its length changing. [`render`] does a whole file
//! offline; [`Cache`] runs it in the background for stretched clips (dev plan 7.2) and keeps
//! the results, so the same input must always give the same output (it does: tested, and the
//! vendored crate seeds its randomness the same way every time).
//!
//! Rendering uses the library's `exact()`, which starts the stretcher on the first input sample
//! and fills exactly the requested output: the result begins where the clip begins, and is
//! `round(length × ratio)` long. `exact()` needs at least twice its output latency of output, so
//! shorter results are rendered padded with silence and cut back.

use crate::media::AudioData;
use anyhow::{bail, Result};
use signalsmith_stretch::Stretch;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// How far a transposition may go, in semitones (two octaves either way).
pub const MAX_SEMITONES: f64 = 24.0;
/// How far a clip may be stretched or shrunk: a quarter to four times its length.
pub const RATIO_RANGE: (f64, f64) = (0.25, 4.0);

/// Whether `ratio` and `semitones` are within what [`render`] accepts; the error says which
/// isn't.
pub fn check(ratio: f64, semitones: f64) -> Result<()> {
    let (low, high) = RATIO_RANGE;
    if !ratio.is_finite() || !(low..=high).contains(&ratio) {
        bail!("stretch ratio {ratio} is outside {low} to {high}");
    }
    if !semitones.is_finite() || semitones.abs() > MAX_SEMITONES {
        bail!("transposition of {semitones} semitones is outside ±{MAX_SEMITONES}");
    }
    Ok(())
}

/// The output length for `input` frames stretched by `ratio`.
pub fn stretched_length(input: usize, ratio: f64) -> usize {
    (input as f64 * ratio).round() as usize
}

/// Stretch and transpose `channels` (planar, all the same length) at `rate` Hz: the result is
/// `ratio` times as long (2.0: twice as long, half the speed) and `semitones` higher, with the
/// pitch otherwise unchanged. Run it at the audio's own sample rate.
pub fn render(
    channels: &[Vec<f32>],
    rate: u32,
    ratio: f64,
    semitones: f64,
) -> Result<Vec<Vec<f32>>> {
    check(ratio, semitones)?;
    let length = channels.first().map_or(0, Vec::len);
    render_warped(
        channels,
        rate,
        &[(length, stretched_length(length, ratio))],
        semitones,
    )
}

/// Stretch `channels` along a warp: consecutive spans of the source, each `(input, output)`
/// turning `input` frames into `output` frames, so the ratio can change along the way (a clip
/// following a tempo map). The spans' inputs must add up to the source's length; the result is
/// as long as their outputs add up to. It is one continuous render, with no seam where the
/// ratio changes. One span is [`render`].
pub fn render_warped(
    channels: &[Vec<f32>],
    rate: u32,
    warp: &[(usize, usize)],
    semitones: f64,
) -> Result<Vec<Vec<f32>>> {
    check(1.0, semitones)?;
    let count = channels.len();
    let length = channels.first().map_or(0, Vec::len);
    if count == 0 || length == 0 {
        return Ok(vec![Vec::new(); count]);
    }
    if channels.iter().any(|c| c.len() != length) {
        bail!("channels of different lengths");
    }
    if warp.iter().map(|s| s.0).sum::<usize>() != length {
        bail!("the warp covers a different length from the audio's {length} frames");
    }
    for &(input, output) in warp {
        if input == 0 || output == 0 {
            bail!("a warp span of {input} -> {output} frames is empty");
        }
        let (low, high) = RATIO_RANGE;
        // The tempo map gives exact ratios like 0.25 that rounding can nudge a hair outside.
        let ratio = output as f64 / input as f64;
        if !(low * 0.999..=high * 1.001).contains(&ratio) {
            bail!("stretch ratio {ratio:.3} is outside {low} to {high}");
        }
    }
    let wanted: usize = warp.iter().map(|s| s.1).sum();
    let mut stretch = Stretch::preset_default(count as u32, rate);
    if semitones != 0.0 {
        // Upstream's advice: a tonality limit keeps more of the timbre when transposing.
        stretch.set_transpose_factor_semitones(semitones as f32, Some(8000.0 / rate as f32));
    }
    let mut warp = merge_short_spans(warp, SHORTEST_SPAN);
    // `exact()` needs at least twice its output latency of output: pad short clips with
    // silence and cut the result back (the start is aligned, so the cut is clean).
    let minimum = 2 * stretch.output_latency() + 1;
    if wanted < minimum {
        let last = warp.last_mut().expect("never empty");
        let ratio = last.1 as f64 / last.0 as f64;
        let needed = minimum - (wanted - last.1);
        let input = ((needed as f64 / ratio).ceil() as usize).max(last.0);
        *last = (input, stretched_length(input, ratio).max(needed));
    }
    let padded_input: usize = warp.iter().map(|s| s.0).sum();
    let padded_output: usize = warp.iter().map(|s| s.1).sum();
    let mut input = vec![0.0f32; padded_input * count];
    for (c, channel) in channels.iter().enumerate() {
        for (i, s) in channel.iter().enumerate() {
            input[i * count + c] = *s;
        }
    }
    let mut output = vec![0.0f32; padded_output * count];
    exact_warped(&mut stretch, &input, count, &warp, &mut output)?;
    Ok((0..count)
        .map(|c| (0..wanted).map(|i| output[i * count + c]).collect())
        .collect())
}

/// How much of an attack a stretch that keeps them plays 1:1: from this long before its onset…
pub const ATTACK_BEFORE_MS: f64 = 30.0;
/// …to this long after. Measured on the DI (`stretch_check`): at 2x, 10 of 11 pick attacks stay
/// as sharp as the original's (4 without), onsets land 2.3 ms off at the median (13.3 without);
/// shorter zones kept fewer, longer ones left less room between close attacks.
pub const ATTACK_AFTER_MS: f64 = 60.0;

/// Spans shorter than this many input frames are folded into the next: short enough to keep
/// an attack's 1:1 zone (`protect_transients`), long enough that rounding stays negligible.
const SHORTEST_SPAN: usize = 64;

/// A warp that keeps attacks unstretched and on time, as élastique does. Each onset (source
/// frames, in order) stays exactly where `warp` puts it, and a zone around it plays 1:1: up to
/// `before` frames ahead of it and `after` past it. Only the audio between zones stretches, to
/// fill the rest, and never by more than twice the span's own ratio (or less than half of it,
/// when shrinking), so close onsets get smaller zones (down to none) rather than overlapping
/// or squeezing the audio between them. Span ends (tempo changes) stay put; a span already at
/// 1:1 is left alone.
pub fn protect_transients(
    warp: &[(usize, usize)],
    onsets: &[usize],
    before: usize,
    after: usize,
) -> Vec<(usize, usize)> {
    let (low, high) = RATIO_RANGE;
    let fits =
        |dx: i64, dy: i64| dx > 0 && dy > 0 && (low..=high).contains(&(dy as f64 / dx as f64));
    let mut points: Vec<(i64, i64)> = vec![(0, 0)];
    let (mut at_in, mut at_out) = (0i64, 0i64);
    for &(input, output) in warp {
        let (start, end) = (
            (at_in, at_out),
            (at_in + input as i64, at_out + output as i64),
        );
        (at_in, at_out) = end;
        let ratio = output as f64 / input as f64;
        if input == output {
            points.push(end);
            continue;
        }
        // Where the span puts a source frame.
        let map = |x: i64| start.1 + ((x - start.0) as f64 * ratio).round() as i64;
        // How much 1:1 a gap of `d` frames can hold with what's left of it stretched at most
        // to `limit`: (ratio·d − s) / (d − s) = limit, solved for s.
        let limit = if ratio > 1.0 {
            (ratio * 2.0).min(high)
        } else {
            (ratio / 2.0).max(low)
        };
        let budget = |d: i64| ((limit - ratio) * d as f64 / (limit - 1.0)).max(0.0) as i64;
        let inside: Vec<i64> = onsets
            .iter()
            .map(|&p| p as i64)
            .filter(|&p| p > start.0 && p < end.0)
            .collect();
        // Each onset's zone: `before` and `after`, shared out over the gaps either side.
        let share = |s: i64| s * before as i64 / (before + after).max(1) as i64;
        let mut zones = Vec::with_capacity(inside.len());
        for (k, &p) in inside.iter().enumerate() {
            let ahead = if k == 0 {
                budget(p - start.0)
            } else {
                share(budget(p - inside[k - 1]))
            };
            let behind = match inside.get(k + 1) {
                Some(&next) => {
                    let s = budget(next - p);
                    s - share(s).min(before as i64)
                }
                None => budget(end.0 - p),
            };
            zones.push((
                p,
                ahead.min(before as i64).max(0),
                behind.min(after as i64).max(0),
            ));
        }
        for (p, b, a) in zones {
            let anchor = map(p);
            let last = *points.last().expect("never empty");
            let (from, to) = ((p - b, anchor - b), (p + a, anchor + a));
            // A zone may begin right where the last one ended (their budget all spent).
            let joins = fits(from.0 - last.0, from.1 - last.1) || from == last;
            if joins && fits(end.0 - to.0, end.1 - to.1) {
                if b > 0 || a > 0 {
                    points.push(from);
                    points.push(to);
                } else {
                    points.push((p, anchor));
                }
            } else if fits(p - last.0, anchor - last.1) && fits(end.0 - p, end.1 - anchor) {
                points.push((p, anchor));
            }
        }
        points.push(end);
    }
    points.dedup();
    points
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0) as usize, (w[1].1 - w[0].1) as usize))
        .collect()
}
/// Spans shorter than `shortest` input frames are folded into the next one (the last into the
/// one before): the stretcher can't change ratio usefully within less than a block, and a
/// handful of frames per call would only add rounding.
fn merge_short_spans(warp: &[(usize, usize)], shortest: usize) -> Vec<(usize, usize)> {
    let mut merged: Vec<(usize, usize)> = Vec::new();
    let mut carry = (0, 0);
    for &(input, output) in warp {
        carry = (carry.0 + input, carry.1 + output);
        if carry.0 >= shortest {
            merged.push(carry);
            carry = (0, 0);
        }
    }
    if carry.0 > 0 {
        match merged.last_mut() {
            Some(last) => *last = (last.0 + carry.0, last.1 + carry.1),
            None => merged.push(carry),
        }
    }
    merged
}

/// Signalsmith's `exact()`, rebuilt from its public steps so the ratio can change between
/// spans: seek on the first input latency's worth, `process` the rest (zero-padded) one span
/// at a time, fold the start back, shift out the output latency and flush the end. With one
/// span it is `exact()` exactly (tested bit for bit). A span boundary goes at the same input
/// and output frame counts in the stream as in the warp: measured with clicks, that lands
/// them within a sample of where the warp puts them (shifting it by either latency missed by
/// up to 1200 frames).
fn exact_warped(
    stretch: &mut Stretch,
    input: &[f32],
    count: usize,
    warp: &[(usize, usize)],
    output: &mut [f32],
) -> Result<()> {
    let frames_in = input.len() / count;
    let frames_out = output.len() / count;
    let (il, ol) = (stretch.input_latency(), stretch.output_latency());
    if frames_out < 2 * ol {
        bail!("the stretcher refused a {frames_out}-frame render");
    }
    let first = warp[0];
    stretch.seek(
        &input[..il.min(frames_in) * count],
        first.0 as f64 / first.1 as f64,
    );
    // `exact()` processes the input from its input latency on, zero-padded at the end.
    let mut stream = vec![0.0f32; frames_in * count];
    if frames_in > il {
        stream[..(frames_in - il) * count].copy_from_slice(&input[il * count..]);
    }
    let (mut done_in, mut done_out) = (0usize, 0usize);
    let (mut at_in, mut at_out) = (0usize, 0usize);
    for (k, &(span_in, span_out)) in warp.iter().enumerate() {
        at_in += span_in;
        at_out += span_out;
        let (to_in, to_out) = if k + 1 == warp.len() {
            (frames_in, frames_out)
        } else {
            (at_in.min(frames_in), at_out.min(frames_out))
        };
        stretch.process(
            &stream[done_in * count..to_in * count],
            &mut output[done_out * count..to_out * count],
        );
        (done_in, done_out) = (to_in, to_out);
    }
    // Fold the first bit back onto itself, and shift out the output latency.
    for c in 0..count {
        for i in 0..(frames_out - ol).min(ol) {
            output[(i + ol) * count + c] -= output[(ol - 1 - i) * count + c];
        }
    }
    output.copy_within(ol * count.., 0);
    stretch.flush(&mut output[(frames_out - ol) * count..]);
    Ok(())
}

/// One render: a whole file (already at the project's rate) along a warp (one span for a
/// plain ratio, one per tempo for a clip following the tempo map) and transposed. Clips render
/// their take's whole file, not just the part they play, so the pieces of a split clip share
/// one render and join without a seam.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub path: PathBuf,
    pub warp: Vec<(usize, usize)>,
    semitones: u64,
    pub rate: u32,
}

impl Key {
    pub fn new(path: PathBuf, warp: Vec<(usize, usize)>, semitones: f64, rate: u32) -> Self {
        Self {
            path,
            warp,
            semitones: semitones.to_bits(),
            rate,
        }
    }

    pub fn semitones(&self) -> f64 {
        f64::from_bits(self.semitones)
    }
}

/// Where a render stands.
#[derive(Debug)]
pub enum Entry {
    /// Queued or running; clearing the flag makes the worker skip it if it hasn't started.
    Pending(Arc<AtomicBool>),
    Ready(Arc<AudioData>),
    Failed(String),
}

struct Job {
    key: Key,
    source: Arc<AudioData>,
    wanted: Arc<AtomicBool>,
}

type Done = (Key, Result<Arc<AudioData>, String>);

/// Stretched renders, made on one background thread and kept while clips use them. Main
/// thread only. Renders are deterministic, so one made again (after an undo, or when a project
/// is reopened) is the same audio.
///
/// One worker, at normal priority, so rendering takes at most one core from the audio threads.
pub struct Cache {
    entries: HashMap<Key, Entry>,
    jobs: Option<Sender<Job>>,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
}

impl Default for Cache {
    fn default() -> Self {
        let (done_tx, done_rx) = channel();
        Self {
            entries: HashMap::new(),
            jobs: None,
            done_tx,
            done_rx,
        }
    }
}

impl Cache {
    pub fn entry(&self, key: &Key) -> Option<&Entry> {
        self.entries.get(key)
    }

    /// The render, if it is ready.
    pub fn get(&self, key: &Key) -> Option<&Arc<AudioData>> {
        match self.entries.get(key) {
            Some(Entry::Ready(data)) => Some(data),
            _ => None,
        }
    }

    /// Start rendering `source` for `key` unless it is already there, pending or failed.
    pub fn request(&mut self, key: &Key, source: &Arc<AudioData>) {
        if self.entries.contains_key(key) {
            return;
        }
        let wanted = Arc::new(AtomicBool::new(true));
        let job = Job {
            key: key.clone(),
            source: source.clone(),
            wanted: wanted.clone(),
        };
        let sent = self.worker().send(job).is_ok();
        self.entries.insert(
            key.clone(),
            if sent {
                Entry::Pending(wanted)
            } else {
                Entry::Failed("the render thread has stopped".into())
            },
        );
    }

    fn worker(&mut self) -> &Sender<Job> {
        self.jobs.get_or_insert_with(|| {
            let (tx, rx) = channel::<Job>();
            let done = self.done_tx.clone();
            let spawned = std::thread::Builder::new()
                .name("stretch".into())
                .spawn(move || {
                    for job in rx {
                        if !job.wanted.load(Ordering::Relaxed) {
                            continue;
                        }
                        let rendered = render_warped(
                            &job.source.channels,
                            job.source.sample_rate,
                            &job.key.warp,
                            job.key.semitones(),
                        )
                        .map(|channels| Arc::new(AudioData::new(job.source.sample_rate, channels)))
                        .map_err(|e| format!("{e:#}"));
                        if done.send((job.key, rendered)).is_err() {
                            break;
                        }
                    }
                });
            if let Err(e) = spawned {
                log::error!("could not start the stretch thread: {e}");
            }
            tx
        })
    }

    /// Take finished renders. Returns whether any arrived (the graph should be rebuilt) and
    /// the failures, to report.
    pub fn poll(&mut self) -> (bool, Vec<String>) {
        let mut arrived = false;
        let mut failures = Vec::new();
        while let Ok((key, result)) = self.done_rx.try_recv() {
            // A render nobody waits for any more (evicted meanwhile) is dropped.
            if !matches!(self.entries.get(&key), Some(Entry::Pending(_))) {
                continue;
            }
            arrived = true;
            let entry = match result {
                Ok(data) => Entry::Ready(data),
                Err(e) => {
                    failures.push(format!("stretching {}: {e}", key.path.display()));
                    Entry::Failed(e)
                }
            };
            self.entries.insert(key, entry);
        }
        (arrived, failures)
    }

    /// How many renders are queued or running.
    pub fn pending(&self) -> usize {
        self.entries
            .values()
            .filter(|e| matches!(e, Entry::Pending(_)))
            .count()
    }

    /// Drop renders `keep` says no clip uses any more; queued ones are skipped.
    pub fn retain(&mut self, keep: impl Fn(&Key) -> bool) {
        self.entries.retain(|key, entry| {
            let kept = keep(key);
            if let (false, Entry::Pending(wanted)) = (kept, &*entry) {
                wanted.store(false, Ordering::Relaxed);
            }
            kept
        });
    }

    pub fn clear(&mut self) {
        self.retain(|_| false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::{num_complex::Complex, FftPlanner};
    use std::f64::consts::PI;

    const RATE: u32 = 48_000;

    fn sine(freq: f64, seconds: f64, amplitude: f64) -> Vec<f32> {
        (0..(seconds * f64::from(RATE)) as usize)
            .map(|i| (amplitude * (2.0 * PI * freq * i as f64 / f64::from(RATE)).sin()) as f32)
            .collect()
    }

    /// The strongest frequency in the middle half of `signal` (Hann window, FFT, parabolic
    /// interpolation between bins).
    fn peak_frequency(signal: &[f32]) -> f64 {
        let n = 32_768.min(signal.len() / 2).next_power_of_two() / 2;
        let start = (signal.len() - n) / 2;
        let mut buffer: Vec<Complex<f64>> = signal[start..start + n]
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos();
                Complex::new(f64::from(*s) * w, 0.0)
            })
            .collect();
        FftPlanner::new().plan_fft_forward(n).process(&mut buffer);
        let mags: Vec<f64> = buffer[..n / 2]
            .iter()
            .map(|c| c.norm().max(1e-12).ln())
            .collect();
        let k = (1..mags.len() - 1)
            .max_by(|a, b| mags[*a].total_cmp(&mags[*b]))
            .unwrap();
        let (a, b, c) = (mags[k - 1], mags[k], mags[k + 1]);
        let shift = 0.5 * (a - c) / (a - 2.0 * b + c);
        (k as f64 + shift) * f64::from(RATE) / n as f64
    }

    fn rms(signal: &[f32]) -> f64 {
        let middle = &signal[signal.len() / 4..signal.len() * 3 / 4];
        (middle.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / middle.len() as f64).sqrt()
    }

    #[test]
    fn length_is_exact_and_pitch_and_level_stay() {
        let tone = sine(440.0, 2.0, 0.5);
        for ratio in [0.5, 0.8, 1.25, 2.0] {
            let out = render(std::slice::from_ref(&tone), RATE, ratio, 0.0).unwrap();
            assert_eq!(
                out[0].len(),
                stretched_length(tone.len(), ratio),
                "ratio {ratio}"
            );
            let f = peak_frequency(&out[0]);
            assert!((f - 440.0).abs() < 0.5, "ratio {ratio}: {f:.2} Hz");
            let db = 20.0 * (rms(&out[0]) / rms(&tone)).log10();
            assert!(db.abs() < 0.5, "ratio {ratio}: level {db:+.2} dB");
        }
    }

    /// RMS of `signal[from..to]` against the middle half, in dB.
    fn level_db(signal: &[f32], from: usize, to: usize) -> f64 {
        let part = &signal[from..to];
        let r =
            (part.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / part.len() as f64).sqrt();
        20.0 * (r / rms(signal)).log10()
    }

    #[test]
    fn the_ends_of_a_steady_tone_keep_their_level() {
        let tone = sine(440.0, 2.0, 0.5);
        let edge = RATE as usize / 20;
        for ratio in [0.5, 2.0] {
            let out = &render(std::slice::from_ref(&tone), RATE, ratio, 0.0).unwrap()[0];
            let (head, tail) = (
                level_db(out, 0, edge),
                level_db(out, out.len() - edge, out.len()),
            );
            println!("ratio {ratio}: first 50 ms {head:+.2} dB, last 50 ms {tail:+.2} dB");
            // The stretcher's window reaches past the clip into silence at each end, so the
            // ends dip a little (measured: up to -1.8 dB at 2x). 7.2 can feed it the audio
            // around the clip instead (TD74).
            assert!(head.abs() < 2.5 && tail.abs() < 2.5, "ratio {ratio}");
        }
    }

    #[test]
    fn the_ends_of_the_ratio_range_render() {
        // Only 0.5-2x is measured for quality; the range's ends just have to work.
        let tone = sine(440.0, 2.0, 0.5);
        for ratio in [RATIO_RANGE.0, RATIO_RANGE.1] {
            let out = &render(std::slice::from_ref(&tone), RATE, ratio, 0.0).unwrap()[0];
            assert_eq!(
                out.len(),
                stretched_length(tone.len(), ratio),
                "ratio {ratio}"
            );
            let db = 20.0 * (rms(out) / rms(&tone)).log10();
            println!(
                "ratio {ratio}: {:.2} Hz, level {db:+.2} dB",
                peak_frequency(out)
            );
            assert!(db.abs() < 3.0, "ratio {ratio}: level {db:+.2} dB");
        }
    }

    #[test]
    fn transposing_moves_the_pitch_not_the_length() {
        let tone = sine(440.0, 2.0, 0.5);
        for (semitones, expected) in [(12.0, 880.0), (-7.0, 440.0 * 2f64.powf(-7.0 / 12.0))] {
            let out = render(std::slice::from_ref(&tone), RATE, 1.0, semitones).unwrap();
            assert_eq!(out[0].len(), tone.len());
            let f = peak_frequency(&out[0]);
            assert!(
                (f - expected).abs() < 1.0,
                "{semitones} st: {f:.2} Hz, expected {expected:.2}"
            );
        }
    }

    #[test]
    fn channels_stay_apart() {
        let (left, right) = (sine(440.0, 2.0, 0.5), sine(660.0, 2.0, 0.5));
        let out = render(&[left, right], RATE, 1.5, 0.0).unwrap();
        assert!((peak_frequency(&out[0]) - 440.0).abs() < 0.5);
        assert!((peak_frequency(&out[1]) - 660.0).abs() < 0.5);
    }

    /// Where a burst after silence first passes a tenth of its level, in samples.
    fn onset(signal: &[f32]) -> usize {
        let peak = signal.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        signal.iter().position(|s| s.abs() > peak * 0.1).unwrap()
    }

    #[test]
    fn an_onset_lands_at_its_stretched_time() {
        // 300 ms of silence, then a decaying noise burst (a drum-like hit).
        let at = 14_400;
        let mut x = 0x1234_5678u32;
        let mut signal = vec![0.0f32; 48_000];
        for (i, s) in signal[at..].iter_mut().enumerate() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = x as f32 / u32::MAX as f32 * 2.0 - 1.0;
            *s = noise * 0.8 * (-(i as f32) / 4_800.0).exp();
        }
        for ratio in [0.5, 0.8, 1.25, 2.0] {
            let out = render(std::slice::from_ref(&signal), RATE, ratio, 0.0).unwrap();
            let expected = (at as f64 * ratio) as i64;
            let error_ms = (onset(&out[0]) as i64 - expected) as f64 * 1000.0 / f64::from(RATE);
            println!("ratio {ratio}: onset {error_ms:+.1} ms from {}", expected);
            // The stretcher works in overlapping blocks, so a hit is smeared a little before its
            // time; measured within a block's reach, not on the sample.
            assert!(
                error_ms.abs() < 20.0,
                "ratio {ratio}: onset off by {error_ms:.1} ms"
            );
        }
    }

    /// `render` the way 7.1 did it: through the library's own `exact()`.
    fn through_exact(channels: &[Vec<f32>], ratio: f64, semitones: f64) -> Vec<Vec<f32>> {
        let count = channels.len();
        let length = channels[0].len();
        let wanted = stretched_length(length, ratio);
        let mut stretch = Stretch::preset_default(count as u32, RATE);
        if semitones != 0.0 {
            stretch.set_transpose_factor_semitones(semitones as f32, Some(8000.0 / RATE as f32));
        }
        let minimum = 2 * stretch.output_latency() + 1;
        let padded_input = if wanted < minimum {
            ((minimum as f64 / ratio).ceil() as usize).max(length)
        } else {
            length
        };
        let padded_output =
            stretched_length(padded_input, ratio).max(minimum.min(wanted.max(minimum)));
        let mut input = vec![0.0f32; padded_input * count];
        for (c, channel) in channels.iter().enumerate() {
            for (i, s) in channel.iter().enumerate() {
                input[i * count + c] = *s;
            }
        }
        let mut output = vec![0.0f32; padded_output * count];
        assert!(stretch.exact(&input, &mut output));
        (0..count)
            .map(|c| (0..wanted).map(|i| output[i * count + c]).collect())
            .collect()
    }

    /// Where a warp puts each source frame in `xs`, at span ends and inside spans.
    fn warped(warp: &[(usize, usize)], x: usize) -> f64 {
        let (mut at_in, mut at_out) = (0, 0);
        for &(input, output) in warp {
            if x <= at_in + input {
                return at_out as f64 + (x - at_in) as f64 * output as f64 / input as f64;
            }
            at_in += input;
            at_out += output;
        }
        at_out as f64
    }

    #[test]
    fn kept_attacks_stay_on_time_and_play_unstretched() {
        let rate = 48_000.0;
        let ms = |v: f64| (v / 1000.0 * rate) as usize;
        let (before, after) = (ms(30.0), ms(60.0));
        let length = 480_000;
        // Attacks far apart, then two only 70 ms apart.
        let onsets = [48_000, 144_000, 240_000, 240_000 + ms(70.0), 400_000];
        for ratio in [2.0, 0.5, 1.37] {
            let base = [(length, stretched_length(length, ratio))];
            let warp = protect_transients(&base, &onsets, before, after);
            assert_eq!(warp.iter().map(|s| s.0).sum::<usize>(), length);
            assert_eq!(warp.iter().map(|s| s.1).sum::<usize>(), base[0].1);
            for &p in &onsets {
                let expected = (p as f64 * ratio).round();
                assert_eq!(
                    warped(&warp, p),
                    expected,
                    "ratio {ratio}: onset {p} off time"
                );
            }
            for &(input, output) in &warp {
                let r = output as f64 / input as f64;
                assert!(
                    (RATIO_RANGE.0..=RATIO_RANGE.1).contains(&r),
                    "ratio {ratio}: a span at {r}"
                );
            }
            // The far-apart attacks play 1:1 around them; the close pair still has some 1:1.
            let unstretched = |p: usize| {
                let (mut at, mut found) = (0, false);
                for &(input, output) in &warp {
                    if p >= at && p < at + input && input == output {
                        found = true;
                    }
                    at += input;
                }
                found
            };
            assert!(unstretched(48_000) && unstretched(144_000) && unstretched(400_000));
            assert!(
                unstretched(240_000) || unstretched(240_000 + ms(70.0)),
                "ratio {ratio}"
            );
        }
        // Nothing to keep, or nothing stretched: the warp as it was.
        let base = [(length, 2 * length)];
        assert_eq!(protect_transients(&base, &[], before, after), base.to_vec());
        let flat = [(length, length)];
        assert_eq!(
            protect_transients(&flat, &onsets, before, after),
            flat.to_vec()
        );
        // A tempo change (two spans): its boundary stays, and each attack keeps the time its own
        // span gives it.
        let two = [(240_000, 480_000), (240_000, 192_000)];
        let warp = protect_transients(&two, &onsets, before, after);
        assert_eq!(warped(&warp, 240_000), 480_000.0);
        assert_eq!(warped(&warp, 400_000), 480_000.0 + 160_000.0 * 0.8);
    }
    #[test]
    fn one_span_is_the_librarys_exact_bit_for_bit() {
        let tone = vec![sine(440.0, 1.0, 0.5), sine(660.0, 1.0, 0.3)];
        for (ratio, semitones) in [(2.0, 0.0), (0.8, 0.0), (1.3, 3.0), (3.0, 0.0)] {
            let ours = render(&tone, RATE, ratio, semitones).unwrap();
            let theirs = through_exact(&tone, ratio, semitones);
            assert!(ours == theirs, "ratio {ratio}, {semitones} st");
        }
        let short = vec![sine(440.0, 0.1, 0.5)];
        assert!(render(&short, RATE, 0.5, 0.0).unwrap() == through_exact(&short, 0.5, 0.0));
    }

    /// Clicks every 100 ms through a warp of 1x then 2x (a tempo halving mid-file).
    #[test]
    fn a_warp_puts_every_click_where_it_says_in_one_render() {
        let length = 4 * RATE as usize;
        let half = length / 2;
        let every = RATE as usize / 10;
        let mut clicks = vec![0.0f32; length];
        for p in (every / 2..length).step_by(every) {
            clicks[p] = 0.9;
        }
        let out =
            &render_warped(&[clicks], RATE, &[(half, half), (half, half * 2)], 0.0).unwrap()[0];
        assert_eq!(out.len(), half * 3);
        for p in (every / 2..length).step_by(every) {
            let expected = if p < half { p } else { half + (p - half) * 2 };
            let window = expected.saturating_sub(1200)..(expected + 1200).min(out.len());
            let found = window
                .max_by(|a, b| out[*a].abs().total_cmp(&out[*b].abs()))
                .unwrap();
            assert!(
                found.abs_diff(expected) <= 2,
                "click at {p}: {found}, not {expected}"
            );
        }
    }

    /// A tone through the same change: in tune on both sides, and no dip or bump where the
    /// ratio changes (separate renders joined there would show one).
    #[test]
    fn a_tone_crosses_a_ratio_change_smoothly() {
        let tone = sine(440.0, 4.0, 0.5);
        let half = tone.len() / 2;
        let out = &render_warped(
            std::slice::from_ref(&tone),
            RATE,
            &[(half, half), (half, half * 2)],
            0.0,
        )
        .unwrap()[0];
        assert!((peak_frequency(&out[..half]) - 440.0).abs() < 1.0);
        assert!((peak_frequency(&out[half..]) - 440.0).abs() < 1.0);
        let window = RATE as usize / 100;
        let steady = rms(&tone);
        for start in (half - 10 * window..half + 10 * window).step_by(window) {
            let part = &out[start..start + window];
            let level =
                (part.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / window as f64).sqrt();
            let db = 20.0 * (level / steady).log10();
            assert!(db.abs() < 1.0, "at {start}: {db:+.2} dB");
        }
    }
    #[test]
    fn the_cache_renders_in_the_background_and_skips_what_nobody_wants() {
        let source = Arc::new(AudioData::new(RATE, vec![sine(440.0, 1.0, 0.5)]));
        let frames = RATE as usize;
        let (a, b) = (
            Key::new("a.wav".into(), vec![(frames, frames * 2)], 0.0, RATE),
            Key::new("a.wav".into(), vec![(frames, frames * 3)], 0.0, RATE),
        );
        let mut cache = Cache::default();
        cache.request(&a, &source);
        // Changed its mind at once: `a` is let go, `b` wanted.
        cache.retain(|k| *k == b);
        cache.request(&b, &source);
        cache.request(&b, &source);
        assert_eq!(cache.pending(), 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while cache.pending() > 0 && std::time::Instant::now() < deadline {
            let (_, failures) = cache.poll();
            assert!(failures.is_empty());
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Whether or not `a` had started, its result is dropped.
        std::thread::sleep(std::time::Duration::from_millis(50));
        cache.poll();
        assert!(cache.entry(&a).is_none());
        let ready = cache.get(&b).expect("b rendered");
        assert_eq!(ready.len(), stretched_length(RATE as usize, 3.0));
        assert!(ready.channels[0] == render(&source.channels, RATE, 3.0, 0.0).unwrap()[0]);
        cache.clear();
        assert!(cache.get(&b).is_none());
    }

    #[test]
    fn the_same_input_always_renders_the_same() {
        // Beyond 2x the library randomises phases: the vendored crate seeds that the same way
        // every time (vendor/signalsmith-stretch/VENDORED.md).
        let tone = sine(330.0, 1.0, 0.4);
        for ratio in [1.3, 3.0] {
            let a = render(&[tone.clone(), tone.clone()], RATE, ratio, 2.0).unwrap();
            let b = render(&[tone.clone(), tone.clone()], RATE, ratio, 2.0).unwrap();
            assert!(
                a.iter()
                    .flatten()
                    .zip(b.iter().flatten())
                    .all(|(x, y)| x.to_bits() == y.to_bits()),
                "ratio {ratio}"
            );
        }
    }

    #[test]
    fn short_clips_come_out_exactly_as_long() {
        let tone = sine(440.0, 0.1, 0.5);
        for ratio in [2.0, 0.5] {
            let out = render(std::slice::from_ref(&tone), RATE, ratio, 0.0).unwrap();
            assert_eq!(
                out[0].len(),
                stretched_length(tone.len(), ratio),
                "ratio {ratio}"
            );
            let f = peak_frequency(&out[0]);
            assert!((f - 440.0).abs() < 5.0, "ratio {ratio}: {f:.2} Hz");
            let db = 20.0 * (rms(&out[0]) / rms(&tone)).log10();
            println!("short clip, ratio {ratio}: {f:.2} Hz, level {db:+.2} dB");
            // 50 ms of output is all ends: the dip of `the_ends_of_a_steady_tone_keep_their_level`.
            assert!(db.abs() < 2.0, "ratio {ratio}: level {db:+.2} dB");
        }
        assert!(render(std::slice::from_ref(&tone), RATE, 5.0, 0.0).is_err());
        assert!(render(&[tone], RATE, 1.0, 30.0).is_err());
        assert_eq!(
            render(&[Vec::new()], RATE, 2.0, 0.0).unwrap(),
            vec![Vec::<f32>::new()]
        );
    }
}
