//! Null testing: subtract two renders and measure what is left.

/// Residual level below which a difference is considered inaudible.
pub const INAUDIBLE_DBFS: f64 = -120.0;

/// Amplitude to dBFS (full scale = 1.0). Silence maps to negative infinity.
pub fn dbfs(x: f64) -> f64 {
    20.0 * x.log10()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    BitIdentical,
    /// Not identical, but the residual peak is below [`INAUDIBLE_DBFS`].
    BelowThreshold,
    Audible,
    /// One of the inputs contains NaN or infinity.
    Invalid,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Verdict::BitIdentical => "bit-identical",
            Verdict::BelowThreshold => "below -120 dBFS (inaudible)",
            Verdict::Audible => "AUDIBLE difference (residual peak above -120 dBFS)",
            Verdict::Invalid => "INVALID (NaN or infinity in the input)",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NullReport {
    /// `b` is compared at `b[n + lag]` against `a[n]`.
    pub lag: isize,
    /// Number of samples per channel in the compared overlap.
    pub compared: usize,
    pub len_a: usize,
    pub len_b: usize,
    pub channels_a: usize,
    pub channels_b: usize,
    /// Largest absolute difference over all channels.
    pub peak: f64,
    /// RMS of the difference over all channels.
    pub rms: f64,
    /// Peak and RMS of `a`, for scale.
    pub ref_peak: f64,
    pub ref_rms: f64,
    /// Number of differing samples.
    pub differing: usize,
    /// Non-finite samples in either input (always a failure).
    pub non_finite: usize,
}

impl NullReport {
    pub fn verdict(&self) -> Verdict {
        let same_shape =
            self.lag == 0 && self.len_a == self.len_b && self.channels_a == self.channels_b;
        if self.non_finite > 0 {
            Verdict::Invalid
        } else if same_shape && self.differing == 0 {
            Verdict::BitIdentical
        } else if self.peak == 0.0 || dbfs(self.peak) < INAUDIBLE_DBFS {
            Verdict::BelowThreshold
        } else {
            Verdict::Audible
        }
    }
}

/// Compare `a[n]` with `b[n + lag]` over the samples both have, channel by channel.
pub fn null(a: &[Vec<f32>], b: &[Vec<f32>], lag: isize) -> NullReport {
    let len_a = a.first().map_or(0, Vec::len);
    let len_b = b.first().map_or(0, Vec::len);
    let (start, end) = overlap(len_a, len_b, lag);
    let mut peak = 0.0f64;
    let mut sum_sq = 0.0f64;
    let mut ref_peak = 0.0f64;
    let mut ref_sq = 0.0f64;
    let mut differing = 0;
    let mut non_finite = 0;
    let channels = a.len().min(b.len());
    for (ca, cb) in a.iter().zip(b) {
        for n in start..end {
            let x = ca[n];
            let y = cb[(n as isize + lag) as usize];
            if !x.is_finite() || !y.is_finite() {
                non_finite += 1;
                continue;
            }
            let d = f64::from(x) - f64::from(y);
            if d != 0.0 {
                differing += 1;
            }
            peak = peak.max(d.abs());
            sum_sq += d * d;
            ref_peak = ref_peak.max(f64::from(x).abs());
            ref_sq += f64::from(x) * f64::from(x);
        }
    }
    let count = ((end - start) * channels).max(1) as f64;
    NullReport {
        lag,
        compared: end - start,
        len_a,
        len_b,
        channels_a: a.len(),
        channels_b: b.len(),
        peak,
        rms: (sum_sq / count).sqrt(),
        ref_peak,
        ref_rms: (ref_sq / count).sqrt(),
        differing,
        non_finite,
    }
}

/// Residual peak and RMS per consecutive segment of `segment` samples (indexed by `a`'s
/// timeline), to show *where* two renders diverge.
pub fn segments(
    a: &[Vec<f32>],
    b: &[Vec<f32>],
    lag: isize,
    segment: usize,
) -> Vec<(usize, f64, f64)> {
    let len_a = a.first().map_or(0, Vec::len);
    let len_b = b.first().map_or(0, Vec::len);
    let (start, end) = overlap(len_a, len_b, lag);
    let segment = segment.max(1);
    (start..end)
        .step_by(segment)
        .map(|s| {
            let e = (s + segment).min(end);
            let mut peak = 0.0f64;
            let mut sq = 0.0f64;
            for (ca, cb) in a.iter().zip(b) {
                for n in s..e {
                    let d = f64::from(ca[n]) - f64::from(cb[(n as isize + lag) as usize]);
                    peak = peak.max(d.abs());
                    sq += d * d;
                }
            }
            let count = ((e - s) * a.len().min(b.len())).max(1) as f64;
            (s, peak, (sq / count).sqrt())
        })
        .collect()
}

/// The range of `n` for which both `a[n]` and `b[n + lag]` exist.
fn overlap(len_a: usize, len_b: usize, lag: isize) -> (usize, usize) {
    let start = (-lag).max(0) as usize;
    let end = (len_b as isize - lag).clamp(0, len_a as isize) as usize;
    (start.min(end), end)
}

/// Mix all channels down to one, for correlation.
pub fn mono(channels: &[Vec<f32>]) -> Vec<f32> {
    let len = channels.first().map_or(0, Vec::len);
    (0..len)
        .map(|i| channels.iter().map(|c| c[i]).sum())
        .collect()
}

/// Find the lag in `-max_lag..=max_lag` at which `b[n + lag]` best matches `a[n]`, by
/// normalized cross-correlation over a window of at most `window` samples that starts at the
/// first non-silent sample of `a`. Returns the lag and its correlation coefficient.
pub fn best_lag(a: &[f32], b: &[f32], max_lag: usize, window: usize) -> (isize, f64) {
    let onset = a.iter().position(|x| x.abs() > 1e-6).unwrap_or(0);
    let win_end = (onset + window).min(a.len());
    let max_lag = max_lag as isize;
    let mut best = (0isize, f64::NEG_INFINITY);
    for lag in -max_lag..=max_lag {
        let (start, end) = overlap(a.len(), b.len(), lag);
        let (start, end) = (start.max(onset), end.min(win_end));
        if start >= end {
            continue;
        }
        let (mut xy, mut xx, mut yy) = (0.0f64, 0.0f64, 0.0f64);
        for n in start..end {
            let x = f64::from(a[n]);
            let y = f64::from(b[(n as isize + lag) as usize]);
            xy += x * y;
            xx += x * x;
            yy += y * y;
        }
        let r = if xx > 0.0 && yy > 0.0 {
            xy / (xx * yy).sqrt()
        } else {
            0.0
        };
        // Prefer the smallest |lag| on ties so identical signals report 0.
        if r > best.1 + 1e-12 || (r >= best.1 - 1e-12 && lag.abs() < best.0.abs()) {
            best = (lag, r);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(len: usize, seed: u64) -> Vec<f32> {
        let mut rng = crate::signal::Rng::new(seed);
        (0..len).map(|_| rng.bipolar() as f32 * 0.5).collect()
    }

    #[test]
    fn identical_signals_are_bit_identical() {
        let a = vec![noise(1000, 1), noise(1000, 2)];
        let r = null(&a, &a.clone(), 0);
        assert_eq!(r.verdict(), Verdict::BitIdentical);
        assert_eq!(r.peak, 0.0);
        assert_eq!(r.compared, 1000);
    }

    #[test]
    fn tiny_difference_is_below_threshold() {
        let a = vec![noise(1000, 1)];
        let mut b = a.clone();
        b[0][500] += 1e-7; // about -140 dBFS
        let r = null(&a, &b, 0);
        assert_eq!(r.differing, 1);
        assert!(dbfs(r.peak) < -130.0 && dbfs(r.peak) > -150.0);
        assert_eq!(r.verdict(), Verdict::BelowThreshold);
    }

    #[test]
    fn half_scale_difference_is_audible_and_measured() {
        let a = vec![vec![0.5f32; 100]];
        let b = vec![vec![0.0f32; 100]];
        let r = null(&a, &b, 0);
        assert_eq!(r.verdict(), Verdict::Audible);
        assert!((dbfs(r.peak) - -6.0206).abs() < 1e-3);
        assert!((dbfs(r.rms) - -6.0206).abs() < 1e-3);
    }

    #[test]
    fn nan_is_never_identical() {
        let a = vec![vec![f32::NAN, 0.0]];
        assert_eq!(null(&a, &a.clone(), 0).verdict(), Verdict::Invalid);
    }

    #[test]
    fn finds_and_cancels_a_shift() {
        let a = noise(20_000, 3);
        let shift = 37;
        // b is a delayed by 37 samples.
        let b: Vec<f32> = std::iter::repeat_n(0.0, shift)
            .chain(a.iter().copied())
            .collect();
        let (lag, r) = best_lag(&a, &b, 100, 8192);
        assert_eq!(lag, shift as isize);
        assert!(r > 0.999);
        let report = null(std::slice::from_ref(&a), std::slice::from_ref(&b), lag);
        assert_eq!(report.differing, 0);
        assert_eq!(report.compared, a.len());
        // And the reverse direction gives the negative lag.
        assert_eq!(best_lag(&b, &a, 100, 8192).0, -(shift as isize));
    }

    #[test]
    fn segments_locate_the_difference() {
        let a = vec![vec![0.0f32; 100]];
        let mut b = a.clone();
        b[0][55] = 0.5;
        let segs = segments(&a, &b, 0, 25);
        assert_eq!(segs.len(), 4);
        let peaks: Vec<f64> = segs.iter().map(|s| s.1).collect();
        assert_eq!(peaks, vec![0.0, 0.0, 0.5, 0.0]);
        assert_eq!(segs[2].0, 50);
    }

    #[test]
    fn overlap_bounds() {
        assert_eq!(overlap(10, 10, 0), (0, 10));
        assert_eq!(overlap(10, 10, 3), (0, 7));
        assert_eq!(overlap(10, 10, -3), (3, 10));
        assert_eq!(overlap(10, 5, 0), (0, 5));
        assert_eq!(overlap(5, 5, 9), (0, 0));
    }
}
