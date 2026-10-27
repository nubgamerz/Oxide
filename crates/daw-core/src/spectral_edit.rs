//! Spectral repair (dev plan 10.5): change a box in time and frequency: attenuate it, remove it, or heal it from the sound around it (spot healing).
//!
//! The change is additive. The audio is cut into Hann-windowed frames (hop a quarter of the
//! frame), and only the bins inside the box are changed in the frames that touch it. The
//! *difference* between the changed and the original spectra is turned back into samples by
//! overlap-add (Hann again, so the windows' squares sum to a constant 1.5) and added to the
//! original. Samples no touched frame reaches stay bit for bit as they were, and outside the
//! box's band the difference is only the windows' leakage.
//!
//! Every frame whose window reaches into the box is changed, so every sample inside the box
//! gets the whole change, even for a click shorter than a hop. Outside the box in time, the
//! change fades out with the windows over one frame (about 43 ms at 48 kHz). The frame grid
//! is anchored at sample 0, so the same box always gives the same result.

use crate::analysis::hann;
use anyhow::{bail, Result};
use rustfft::{num_complex::Complex, FftPlanner};

/// The squares of periodic Hann windows a quarter of their length apart sum to this.
const OVERLAP: f32 = 1.5;
/// How many frames on each side of the box heal takes its sound from.
const HEAL_FRAMES: i64 = 4;
/// How many bins below and above the band heal takes its sound from when the box covers the
/// whole file in time.
const HEAL_BINS: usize = 3;

/// What to do to the box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpectralOp {
    /// Turn it down by `db` (negative: quieter).
    Attenuate { db: f64 },
    /// Take it out.
    Remove,
    /// Replace it with the sound around it: each bin's level is interpolated across the box
    /// from the frames just before and after it (the bins just below and above it when it
    /// spans the whole file), keeping the original phase.
    Heal,
}

/// A box: `start..end` in the file's own samples, `low_hz..high_hz` in frequency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpectralBox {
    pub start: u64,
    pub end: u64,
    pub low_hz: f64,
    pub high_hz: f64,
}

/// The frame length at `rate`: about 43 ms (2048 samples at 44.1 and 48 kHz), fine enough in
/// frequency to separate a guitar's partials and short enough to hold a click.
pub fn frame_size(rate: u32) -> usize {
    (rate as usize / 24).next_power_of_two().max(256)
}

/// Check a box against a file of `len` samples at `rate`.
pub fn check(b: &SpectralBox, len: u64, rate: u32) -> Result<()> {
    let nyquist = f64::from(rate) / 2.0;
    if b.end <= b.start {
        bail!("the box must end after it starts ({}..{})", b.start, b.end);
    }
    if b.end > len {
        bail!(
            "the box {}..{} runs past the file's end ({len} samples)",
            b.start,
            b.end
        );
    }
    if !(b.low_hz >= 0.0 && b.high_hz > b.low_hz) {
        bail!("the band must go up: {} Hz to {} Hz", b.low_hz, b.high_hz);
    }
    if b.high_hz > nyquist {
        bail!(
            "{} Hz is above this file's highest frequency ({nyquist} Hz)",
            b.high_hz
        );
    }
    Ok(())
}

/// `samples` (one channel at `rate`) with the box changed by `op`.
pub fn process(samples: &[f32], rate: u32, b: &SpectralBox, op: SpectralOp) -> Result<Vec<f32>> {
    let len = samples.len() as i64;
    check(b, len as u64, rate)?;
    let n = frame_size(rate);
    let hop = (n / 4) as i64;
    let half = (n / 2) as i64;
    let bin_hz = f64::from(rate) / n as f64;
    // The bins in the band, by their centre frequency, half a bin either side.
    let lo = ((b.low_hz / bin_hz - 0.5).ceil().max(0.0)) as usize;
    let hi = (((b.high_hz / bin_hz) + 0.5).floor() as usize).min(n / 2);
    if hi < lo {
        bail!(
            "{}–{} Hz is narrower than one analysis bin ({bin_hz:.1} Hz)",
            b.low_hz,
            b.high_hz
        );
    }
    // Every frame whose window reaches into the box: centre k·hop with k·hop + n/2 > start
    // and k·hop - n/2 < end. So every sample inside the box gets the whole change.
    let (start, end) = (b.start as i64, b.end as i64);
    let first = (start - half).div_euclid(hop) + 1;
    let last = (end + half - 1).div_euclid(hop);
    let frames: Vec<i64> = (first..=last).collect();

    let mut fft = Stft::new(n);
    let window = hann(n);
    // The changed frames' spectra minus the originals'.
    let mut diffs: Vec<Vec<Complex<f32>>> = Vec::with_capacity(frames.len());
    match op {
        SpectralOp::Attenuate { .. } | SpectralOp::Remove => {
            let g = match op {
                SpectralOp::Attenuate { db } => 10f64.powf(db / 20.0) as f32,
                _ => 0.0,
            };
            for &k in &frames {
                let x = fft.forward(samples, k * hop);
                diffs.push(x[lo..=hi].iter().map(|v| v * (g - 1.0)).collect());
            }
        }
        SpectralOp::Heal => {
            let mags = |fft: &mut Stft, k: i64| -> Vec<f32> {
                fft.forward(samples, k * hop)
                    .iter()
                    .map(|v| v.norm())
                    .collect()
            };
            // Reference frames: wholly inside the file and clear of the box and its frames.
            let clear_before = |k: i64| k * hop + half <= start && k * hop - half >= 0;
            let clear_after = |k: i64| k * hop - half >= end && k * hop + half <= len;
            let mean = |fft: &mut Stft, ks: Vec<i64>| -> Option<(f64, Vec<f32>)> {
                if ks.is_empty() {
                    return None;
                }
                let mut sum = vec![0.0f32; n / 2 + 1];
                for &k in &ks {
                    for (s, m) in sum.iter_mut().zip(mags(fft, k)) {
                        *s += m;
                    }
                }
                let count = ks.len() as f32;
                let at = ks.iter().sum::<i64>() as f64 / ks.len() as f64;
                Some((at, sum.into_iter().map(|s| s / count).collect()))
            };
            let before_last = (start - half).div_euclid(hop);
            let before: Vec<i64> = (before_last - HEAL_FRAMES + 1..=before_last)
                .filter(|&k| clear_before(k))
                .collect();
            let after_first = (end + half + hop - 1).div_euclid(hop);
            let after: Vec<i64> = (after_first..after_first + HEAL_FRAMES)
                .filter(|&k| clear_after(k))
                .collect();
            let left = mean(&mut fft, before);
            let right = mean(&mut fft, after);
            let across_time = left.is_some() || right.is_some();
            // No frames on either side, and no bins below (DC doesn't count) or above.
            if !across_time && lo <= 1 && hi >= n / 2 {
                bail!("the box covers the whole file and every frequency: nothing around it to heal from");
            }
            for &k in &frames {
                let x = fft.forward(samples, k * hop);
                let target: Vec<f32> = match (&left, &right) {
                    (Some((a, l)), Some((z, r))) => {
                        let t = ((k as f64 - a) / (z - a)).clamp(0.0, 1.0) as f32;
                        (lo..=hi).map(|j| l[j] * (1.0 - t) + r[j] * t).collect()
                    }
                    (Some((_, side)), None) | (None, Some((_, side))) => side[lo..=hi].to_vec(),
                    (None, None) => {
                        // Across the band, from the bins just below and above it.
                        let level = |range: std::ops::Range<usize>| -> Option<f32> {
                            let bins: Vec<f32> = range.map(|j| x[j].norm()).collect();
                            (!bins.is_empty()).then(|| bins.iter().sum::<f32>() / bins.len() as f32)
                        };
                        let below = level(lo.saturating_sub(HEAL_BINS).max(1)..lo);
                        let above = level(hi + 1..(hi + 1 + HEAL_BINS).min(n / 2 + 1));
                        let (a, z) = match (below, above) {
                            (Some(a), Some(z)) => (a, z),
                            (Some(v), None) | (None, Some(v)) => (v, v),
                            (None, None) => (0.0, 0.0),
                        };
                        let width = (hi - lo + 1) as f32;
                        (lo..=hi)
                            .map(|j| {
                                let t = (j - lo) as f32 / width;
                                a * (1.0 - t) + z * t
                            })
                            .collect()
                    }
                };
                diffs.push(
                    x[lo..=hi]
                        .iter()
                        .zip(target)
                        .map(|(v, m)| {
                            let r = v.norm();
                            let healed = if r > 1e-20 {
                                v * (m / r)
                            } else {
                                Complex::new(m, 0.0)
                            };
                            healed - v
                        })
                        .collect(),
                );
            }
        }
    }

    // Overlap-add the differences, windowed again, onto the original.
    let mut out = samples.to_vec();
    for (&k, d) in frames.iter().zip(&diffs) {
        let time = fft.inverse(d, lo);
        let first = k * hop - half;
        for (i, (v, w)) in time.iter().zip(&window).enumerate() {
            let at = first + i as i64;
            if (0..len).contains(&at) {
                out[at as usize] += v * w / OVERLAP;
            }
        }
    }
    Ok(out)
}

/// Forward and inverse FFTs of `n`-sample Hann-windowed frames.
struct Stft {
    n: usize,
    forward: std::sync::Arc<dyn rustfft::Fft<f32>>,
    inverse: std::sync::Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    buffer: Vec<Complex<f32>>,
}

impl Stft {
    fn new(n: usize) -> Self {
        let mut planner = FftPlanner::new();
        Self {
            n,
            forward: planner.plan_fft_forward(n),
            inverse: planner.plan_fft_inverse(n),
            window: hann(n),
            buffer: vec![Complex::new(0.0, 0.0); n],
        }
    }

    /// The spectrum (bins 0 to n/2) of the frame centred on `centre`, zero-padded past the ends.
    fn forward(&mut self, samples: &[f32], centre: i64) -> &[Complex<f32>] {
        let first = centre - (self.n / 2) as i64;
        for (k, slot) in self.buffer.iter_mut().enumerate() {
            let v = usize::try_from(first + k as i64)
                .ok()
                .and_then(|i| samples.get(i))
                .copied()
                .unwrap_or(0.0);
            *slot = Complex::new(v * self.window[k], 0.0);
        }
        self.forward.process(&mut self.buffer);
        &self.buffer[..=self.n / 2]
    }

    /// The real frame whose spectrum is `bins` from bin `from` up (zero elsewhere, mirrored so
    /// the result is real), scaled back from rustfft's unnormalised transform.
    fn inverse(&mut self, bins: &[Complex<f32>], from: usize) -> Vec<f32> {
        let n = self.n;
        self.buffer.fill(Complex::new(0.0, 0.0));
        for (i, &v) in bins.iter().enumerate() {
            let j = from + i;
            if j == 0 || j == n / 2 {
                // DC and Nyquist are their own mirrors: only their real part makes a real signal.
                self.buffer[j] = Complex::new(v.re, 0.0);
            } else {
                self.buffer[j] = v;
                self.buffer[n - j] = v.conj();
            }
        }
        self.inverse.process(&mut self.buffer);
        self.buffer.iter().map(|c| c.re / n as f32).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn tones(len: usize, parts: &[(f64, f32)]) -> Vec<f32> {
        (0..len)
            .map(|i| {
                let t = i as f64 / f64::from(RATE);
                parts
                    .iter()
                    .map(|&(hz, a)| a * (2.0 * std::f64::consts::PI * hz * t).sin() as f32)
                    .sum()
            })
            .collect()
    }

    /// The amplitude of the `hz` sine in `samples[range]`, by correlation with sin and cos.
    fn amplitude(samples: &[f32], range: std::ops::Range<usize>, hz: f64) -> f64 {
        let (mut s, mut c) = (0.0, 0.0);
        for i in range.clone() {
            let p = 2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(RATE);
            s += f64::from(samples[i]) * p.sin();
            c += f64::from(samples[i]) * p.cos();
        }
        2.0 * (s * s + c * c).sqrt() / range.len() as f64
    }

    fn db(ratio: f64) -> f64 {
        20.0 * ratio.log10()
    }

    #[test]
    fn removing_every_frequency_leaves_silence_inside_the_box() {
        let x = tones(
            RATE as usize * 2,
            &[(110.0, 0.3), (1234.5, 0.3), (9876.0, 0.2)],
        );
        let b = SpectralBox {
            start: 0,
            end: x.len() as u64,
            low_hz: 0.0,
            high_hz: f64::from(RATE) / 2.0,
        };
        let y = process(&x, RATE, &b, SpectralOp::Remove).unwrap();
        let n = frame_size(RATE);
        let peak = y[n..y.len() - n].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(
            db(f64::from(peak)) < -100.0,
            "left {:.1} dBFS",
            db(f64::from(peak))
        );
    }

    #[test]
    fn attenuating_a_box_over_a_tone_spares_the_rest() {
        let len = RATE as usize * 2;
        let x = tones(len, &[(1000.0, 0.5), (5000.0, 0.5)]);
        let b = SpectralBox {
            start: 24_000,
            end: 72_000,
            low_hz: 800.0,
            high_hz: 1250.0,
        };
        let y = process(&x, RATE, &b, SpectralOp::Attenuate { db: -60.0 }).unwrap();
        let n = frame_size(RATE);
        let inside = 24_000 + n..72_000 - n;
        let drop =
            db(amplitude(&y, inside.clone(), 1000.0) / amplitude(&x, inside.clone(), 1000.0));
        assert!(drop < -40.0, "the tone in the box went down {drop:.1} dB");
        let other = db(amplitude(&y, inside.clone(), 5000.0) / amplitude(&x, inside, 5000.0));
        assert!(
            other.abs() < 0.1,
            "the tone outside the band moved {other:.3} dB"
        );
        // Further than a frame from the box in time, nothing changed at all.
        let reach = n;
        assert_eq!(x[..24_000 - reach], y[..24_000 - reach]);
        assert_eq!(x[72_000 + reach..], y[72_000 + reach..]);
        // Attenuating by 0 dB is no change.
        let same = process(&x, RATE, &b, SpectralOp::Attenuate { db: 0.0 }).unwrap();
        assert_eq!(x, same);
    }

    #[test]
    fn a_click_narrower_than_a_hop_is_still_removed() {
        let len = RATE as usize;
        let clean = tones(len, &[(440.0, 0.3)]);
        let mut x = clean.clone();
        // A 2 ms squeak at 6 kHz in the middle, smoothly shaped (a hard-gated one would
        // spread far outside the box's band).
        let squeak = tones(96, &[(6000.0, 0.5)]);
        for (w, (i, s)) in hann(96).iter().zip(squeak.iter().enumerate()) {
            x[24_000 + i] += s * w;
        }
        let b = SpectralBox {
            start: 24_000,
            end: 24_096,
            low_hz: 4000.0,
            high_hz: 9000.0,
        };
        let y = process(&x, RATE, &b, SpectralOp::Remove).unwrap();
        let err = |v: &[f32]| -> f64 {
            v.iter()
                .zip(&clean)
                .map(|(a, b)| f64::from(a - b).powi(2))
                .sum::<f64>()
                .sqrt()
        };
        let reduced = db(err(&y) / err(&x));
        assert!(reduced < -20.0, "the squeak went down only {reduced:.1} dB");
    }

    #[test]
    fn healing_fills_a_squeak_from_the_sound_around_it() {
        let len = RATE as usize * 2;
        let clean = tones(len, &[(440.0, 0.3), (5000.0, 0.05)]);
        let mut x = clean.clone();
        // A 20 ms burst at 5.5 kHz, Hann-shaped, over the steady 5 kHz partial.
        let burst = 960;
        for i in 0..burst {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / burst as f64).cos();
            let t = i as f64 / f64::from(RATE);
            x[48_000 + i] += (0.5 * w * (2.0 * std::f64::consts::PI * 5500.0 * t).sin()) as f32;
        }
        let b = SpectralBox {
            start: 47_000,
            end: 50_000,
            low_hz: 4500.0,
            high_hz: 6500.0,
        };
        let y = process(&x, RATE, &b, SpectralOp::Heal).unwrap();
        let region = 44_000..54_000;
        let err = |v: &[f32]| -> f64 {
            v[region.clone()]
                .iter()
                .zip(&clean[region.clone()])
                .map(|(a, b)| f64::from(a - b).powi(2))
                .sum::<f64>()
                .sqrt()
        };
        let reduced = db(err(&y) / err(&x));
        assert!(reduced < -20.0, "the burst went down only {reduced:.1} dB");
        // The steady 5 kHz partial it was healed from is still there (within 1 dB).
        let kept = db(amplitude(&y, 48_000..49_000, 5000.0) / 0.05);
        assert!(kept.abs() < 1.0, "the partial moved {kept:.2} dB");
        // And the 440 Hz tone under the band is untouched.
        let low =
            db(amplitude(&y, 47_000..50_000, 440.0) / amplitude(&clean, 47_000..50_000, 440.0));
        assert!(low.abs() < 0.1, "440 Hz moved {low:.3} dB");
    }

    #[test]
    fn healing_the_whole_file_uses_the_bins_around_the_band() {
        let x = tones(RATE as usize, &[(440.0, 0.3), (3000.0, 0.3)]);
        let b = SpectralBox {
            start: 0,
            end: x.len() as u64,
            low_hz: 2500.0,
            high_hz: 3500.0,
        };
        let y = process(&x, RATE, &b, SpectralOp::Heal).unwrap();
        let n = frame_size(RATE);
        let inside = n..x.len() - n;
        let gone = db(amplitude(&y, inside.clone(), 3000.0) / 0.3);
        assert!(gone < -30.0, "3 kHz went down only {gone:.1} dB");
        let all = SpectralBox {
            low_hz: 0.0,
            high_hz: f64::from(RATE) / 2.0,
            ..b
        };
        let err = process(&x, RATE, &all, SpectralOp::Heal).unwrap_err();
        assert!(err.to_string().contains("nothing around it"), "{err}");
    }

    #[test]
    fn bad_boxes_are_refused_by_name() {
        let x = vec![0.0f32; 1000];
        let ok = SpectralBox {
            start: 10,
            end: 500,
            low_hz: 100.0,
            high_hz: 200.0,
        };
        for (b, says) in [
            (SpectralBox { end: 10, ..ok }, "end after"),
            (SpectralBox { end: 2000, ..ok }, "past the file's end"),
            (
                SpectralBox {
                    high_hz: 50.0,
                    ..ok
                },
                "must go up",
            ),
            (
                SpectralBox {
                    high_hz: 30_000.0,
                    ..ok
                },
                "30000 Hz",
            ),
        ] {
            let err = process(&x, RATE, &b, SpectralOp::Remove).unwrap_err();
            assert!(err.to_string().contains(says), "{err}");
        }
    }
}
