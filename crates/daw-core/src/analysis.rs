//! Audio analysis for editing: finding transients (note onsets) in recorded audio.
//!
//! Detection uses spectral flux, the summed increase of each frequency bin's log magnitude
//! from one short frame to the next. A new note raises energy across the spectrum even when
//! the overall level barely changes, which is what a DI guitar or a legato line does.
//! Peaks of the flux above an adaptive threshold (local median plus a margin set by
//! `sensitivity`) are onsets. Each onset is then refined in the time domain to the sample
//! where the attack starts, because a 256-sample frame hop is far too coarse to cut at.

use rustfft::{num_complex::Complex, FftPlanner};

const FRAME: usize = 1024;
const HOP: usize = 256;
/// Frames either side for the local-median threshold.
const MEDIAN_SPAN: usize = 8;
/// Frames quieter than this (RMS, dBFS) never start a transient.
const SILENCE_DB: f32 = -60.0;

/// What to look for.
#[derive(Debug, Clone, Copy)]
pub struct OnsetSettings {
    /// 0 (only the clearest attacks) to 1 (every small change).
    pub sensitivity: f32,
    /// Two transients are never closer than this, in samples.
    pub min_gap: usize,
}

impl OnsetSettings {
    pub fn new(sensitivity: f32, min_gap_ms: f32, sample_rate: u32) -> Self {
        Self {
            sensitivity: sensitivity.clamp(0.0, 1.0),
            min_gap: (min_gap_ms.max(1.0) / 1000.0 * sample_rate as f32) as usize,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Onset {
    /// Sample index in the analysed signal where the attack starts.
    pub position: usize,
    /// How pronounced the change is, 0 to 1 (relative to the strongest in the signal).
    pub strength: f32,
}

/// Find transients in a mono signal.
pub fn detect_onsets(samples: &[f32], settings: OnsetSettings) -> Vec<Onset> {
    if samples.len() < FRAME {
        return Vec::new();
    }
    let raw = spectral_flux(samples);
    let peak = raw.iter().copied().fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return Vec::new();
    }
    let flux: Vec<f32> = raw.iter().map(|f| f / peak).collect();
    // The margin above the local median: high sensitivity lets small rises through.
    let margin = 0.02 + (1.0 - settings.sensitivity) * 0.3;
    // An absolute floor too, because the relative test alone would promote the frame-to-frame
    // wobble of a signal with no attacks at all. Measured: a steady tone stays under 9, note
    // attacks over a ringing guitar reach about 60, anything out of silence hundreds.
    let floor = 15.0 + (1.0 - settings.sensitivity) * 25.0;
    let silence = 10f32.powf(SILENCE_DB / 20.0);

    let mut onsets: Vec<Onset> = Vec::new();
    for i in 1..flux.len() {
        let lo = i.saturating_sub(MEDIAN_SPAN);
        let hi = (i + MEDIAN_SPAN + 1).min(flux.len());
        let threshold = median(&flux[lo..hi]) + margin;
        let local_max = flux[i.saturating_sub(2)..(i + 3).min(flux.len())]
            .iter()
            .all(|f| *f <= flux[i]);
        if flux[i] < threshold || raw[i] < floor || !local_max {
            continue;
        }
        // Frame i compares frames i-1 and i, so the change happens around frame i's start.
        let frame_start = i * HOP;
        let window = &samples[frame_start..(frame_start + FRAME).min(samples.len())];
        if rms(window) < silence {
            continue;
        }
        let position = refine(samples, frame_start);
        match onsets.last_mut() {
            Some(last) if position < last.position + settings.min_gap => {
                // Too close: keep the stronger of the two.
                if flux[i] > last.strength {
                    *last = Onset {
                        position,
                        strength: flux[i],
                    };
                }
            }
            _ => onsets.push(Onset {
                position,
                strength: flux[i],
            }),
        }
    }
    onsets
}

/// Half-wave rectified flux of log-compressed magnitude spectra, one value per hop.
fn spectral_flux(samples: &[f32]) -> Vec<f32> {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FRAME);
    let window = hann(FRAME);
    let frames = (samples.len() - FRAME) / HOP + 1;
    let mut previous = vec![0.0f32; FRAME / 2];
    let mut buffer = vec![Complex::new(0.0f32, 0.0); FRAME];
    let mut flux = Vec::with_capacity(frames);
    for f in 0..frames {
        let start = f * HOP;
        for (k, slot) in buffer.iter_mut().enumerate() {
            *slot = Complex::new(samples[start + k] * window[k], 0.0);
        }
        fft.process(&mut buffer);
        let mut sum = 0.0;
        for (bin, prev) in buffer[..FRAME / 2].iter().zip(previous.iter_mut()) {
            let magnitude = (1.0 + 100.0 * bin.norm()).ln();
            sum += (magnitude - *prev).max(0.0);
            *prev = magnitude;
        }
        // The first frame compares against silence; it is not a real change.
        flux.push(if f == 0 { 0.0 } else { sum });
    }
    flux
}

/// The sample where the attack near `around` begins: find the loudest point of the short
/// envelope just after it, then walk back to where the envelope was still below a quarter
/// of that.
fn refine(samples: &[f32], around: usize) -> usize {
    let from = around.saturating_sub(HOP);
    let to = (around + FRAME).min(samples.len());
    // ~1 ms envelope.
    let smooth = 48;
    let envelope: Vec<f32> = (from..to)
        .map(|i| {
            let lo = i.saturating_sub(smooth / 2);
            let hi = (i + smooth / 2).min(samples.len());
            samples[lo..hi].iter().fold(0.0f32, |m, s| m.max(s.abs()))
        })
        .collect();
    let (peak_at, peak) =
        envelope.iter().enumerate().fold(
            (0, 0.0f32),
            |best, (i, v)| if *v > best.1 { (i, *v) } else { best },
        );
    let mut i = peak_at;
    while i > 0 && envelope[i - 1] >= peak * 0.25 {
        i -= 1;
    }
    from + i
}

fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    sorted[sorted.len() / 2]
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// A periodic Hann window of `n` points, for FFT frames.
pub fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|k| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / n as f32).cos())
        .collect()
}

/// Average channels into one.
pub fn mono(channels: &[Vec<f32>], start: usize, end: usize) -> Vec<f32> {
    let end = end.min(channels.first().map_or(0, Vec::len));
    if start >= end || channels.is_empty() {
        return Vec::new();
    }
    let scale = 1.0 / channels.len() as f32;
    (start..end)
        .map(|i| channels.iter().map(|c| c[i]).sum::<f32>() * scale)
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A plucked-string stand-in: a short noise burst into a decaying tone.
    pub fn pluck(out: &mut [f32], at: usize, freq: f32, rate: f32, seed: u32) {
        let mut x = seed.max(1);
        for (n, s) in out[at..].iter_mut().enumerate().take(rate as usize) {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = (x as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let t = n as f32 / rate;
            let burst = if n < 96 { noise * 0.5 } else { 0.0 };
            *s += 0.5 * (-t * 6.0).exp() * (2.0 * std::f32::consts::PI * freq * t).sin() + burst;
        }
    }

    /// Notes at uneven, known positions over a quiet noise floor.
    pub fn performance(rate: u32) -> (Vec<f32>, Vec<usize>) {
        let r = rate as f32;
        let starts: Vec<usize> = [0.25, 0.71, 1.02, 1.49, 1.77, 2.26, 2.52, 3.01]
            .iter()
            .map(|s| (s * r) as usize)
            .collect();
        let mut signal = vec![0.0f32; (3.6 * r) as usize];
        let freqs = [82.4, 110.0, 146.8, 196.0, 110.0, 164.8, 220.0, 98.0];
        for (k, (&at, &f)) in starts.iter().zip(&freqs).enumerate() {
            pluck(&mut signal, at, f, r, k as u32 + 7);
        }
        // A noise floor that must not trigger anything.
        let mut x = 99u32;
        for s in &mut signal {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *s += ((x as f32 / u32::MAX as f32) * 2.0 - 1.0) * 0.001;
        }
        (signal, starts)
    }

    #[test]
    fn finds_every_note_to_within_a_millisecond() {
        let rate = 48_000;
        let (signal, starts) = performance(rate);
        let onsets = detect_onsets(&signal, OnsetSettings::new(0.5, 50.0, rate));
        let found: Vec<usize> = onsets.iter().map(|o| o.position).collect();
        assert_eq!(
            found.len(),
            starts.len(),
            "found {found:?}, expected {starts:?}"
        );
        for (f, s) in found.iter().zip(&starts) {
            assert!(f.abs_diff(*s) <= 48, "onset at {f}, note at {s}");
        }
    }

    #[test]
    fn silence_and_steady_tones_have_no_transients() {
        let rate = 48_000;
        assert!(detect_onsets(&vec![0.0; 96_000], OnsetSettings::new(1.0, 50.0, rate)).is_empty());
        let tone: Vec<f32> = (0..96_000)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 48_000.0).sin())
            .collect();
        let onsets = detect_onsets(&tone[..], OnsetSettings::new(0.5, 50.0, rate));
        assert!(
            onsets.is_empty(),
            "a steady tone has no attacks: {onsets:?}"
        );
    }

    #[test]
    fn quiet_notes_are_still_found() {
        let mut quiet = vec![0.0f32; 96_000];
        pluck(&mut quiet, 48_000, 110.0, 48_000.0, 3);
        quiet.iter_mut().for_each(|s| *s *= 0.01); // -40 dB
        let onsets = detect_onsets(&quiet, OnsetSettings::new(0.5, 50.0, 48_000));
        assert_eq!(onsets.len(), 1, "{onsets:?}");
        assert!(onsets[0].position.abs_diff(48_000) <= 48);
    }

    #[test]
    fn sensitivity_and_gap_control_how_many() {
        let rate = 48_000;
        let (signal, starts) = performance(rate);
        let all = detect_onsets(&signal, OnsetSettings::new(0.5, 50.0, rate)).len();
        // A gap longer than the closest pair of notes merges them.
        let sparse = detect_onsets(&signal, OnsetSettings::new(0.5, 400.0, rate)).len();
        assert!(
            sparse < all && sparse >= starts.len() / 2,
            "{sparse} vs {all}"
        );
    }
}
