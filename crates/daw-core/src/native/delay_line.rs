//! What the delays and modulation effects (dev plan 11.7: the Delay, the Echo, the Analog
//! Delay, the Chorus, the Flanger) share: a ring of samples sized once for the longest delay at
//! the highest rate, read back by whole samples, by a four-point cubic, by the Chorus's
//! Catmull-Rom Hermite or by a 32-tap windowed sinc ([`Sinc`]), and rewritten in place where a
//! feedback goes back into it; a parameter moving in a straight line; and a crossfade from one
//! set of settings to the next, forwards.
//!
//! Nothing here allocates after construction, and emptying a line costs nothing: it forgets
//! what it holds by counting the samples written since (reads further back give silence), so
//! a transport start doesn't sweep megabytes on the audio thread (the Vintage Delay keeps 16 s a
//! channel).

/// A ring of samples: the newest written last, read back by age (1: the last written).
pub struct Line {
    buf: Vec<f32>,
    /// The samples in use (at most the capacity): the oldest age that can be read.
    len: usize,
    /// Where the next sample goes.
    pos: usize,
    /// Samples written since the line was emptied (at most `len`): older ages read silence.
    filled: usize,
}

impl Line {
    /// A line with room for `capacity` samples, `capacity` of them in use, empty.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            buf: vec![0.0; capacity],
            len: capacity,
            pos: 0,
            filled: 0,
        }
    }

    /// Use `len` samples of it (at most its capacity), empty. No allocation, no sweep.
    pub fn reset(&mut self, len: usize) {
        self.len = len.clamp(1, self.buf.len());
        self.clear();
    }

    /// Empty: everything written so far reads as silence from now on.
    pub fn clear(&mut self) {
        self.pos = 0;
        self.filled = 0;
    }

    /// Append the newest sample (the oldest in use drops out).
    #[inline]
    pub fn write(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos += 1;
        if self.pos == self.len {
            self.pos = 0;
        }
        if self.filled < self.len {
            self.filled += 1;
        }
    }

    /// The sample written `age` writes ago (1: the last one), or silence if it was written
    /// before the line was last emptied, or never (also for 0 and ages past the length).
    #[inline]
    pub fn back(&self, age: usize) -> f32 {
        if age == 0 || age > self.filled {
            return 0.0;
        }
        let i = if self.pos >= age {
            self.pos - age
        } else {
            self.pos + self.len - age
        };
        self.buf[i]
    }

    /// The signal `age` samples back (fractional, at least 3 so all four points exist), by the
    /// common four-point cubic (not Hermite) through the samples either side: with `y0` at the
    /// whole age at or above it, `y1` and `y2` the two newer, `ym` the older, and `μ` how far
    /// towards `y1`, `a = y2 − y1 − ym + y0` and `a·μ³ + (ym − y0 − a)·μ² + (y1 − ym)·μ + y0`,
    /// all f32.
    #[inline]
    pub fn cubic(&self, age: f64) -> f32 {
        let whole = age.ceil();
        let mu = (whole - age) as f32;
        let k = whole as usize;
        let (ym, y0, y1, y2) = (
            self.back(k + 1),
            self.back(k),
            self.back(k.saturating_sub(1)),
            self.back(k.saturating_sub(2)),
        );
        let a = y2 - y1 - ym + y0;
        ((a * mu + (ym - y0 - a)) * mu + (y1 - ym)) * mu + y0
    }

    /// The signal `age` samples back (fractional, at least 2) by the cubic Hermite with
    /// Catmull-Rom tangents, as the Chorus reads its voices: between `y0` (the whole age
    /// at or above it) and `y1` (one newer), the tangents half the differences across them
    /// (`m0 = (y1 − ym)/2`, `m1 = (y2 − y0)/2`), at `t` towards `y1`:
    /// `t³(2y0 − 2y1 + m0 + m1) + t²(3(y1 − y0) − 2m0 − m1) + m0·t + y0`, all f32.
    #[inline]
    pub fn hermite(&self, age: f64) -> f32 {
        let whole = age.ceil();
        let t = (whole - age) as f32;
        let k = whole as usize;
        // The four samples straight from the buffer when they lie in one stretch of it.
        let oldest = k + 1;
        let first = if self.pos >= oldest {
            self.pos.wrapping_sub(oldest)
        } else {
            (self.pos + self.len).wrapping_sub(oldest)
        };
        let (ym, y0, y1, y2) = if k >= 3 && oldest <= self.filled && first + 4 <= self.len {
            let s = &self.buf[first..first + 4];
            (s[0], s[1], s[2], s[3])
        } else {
            (
                self.back(k + 1),
                self.back(k),
                self.back(k.saturating_sub(1)),
                self.back(k.saturating_sub(2)),
            )
        };
        let m0 = 0.5 * (y1 - ym);
        let m1 = 0.5 * (y2 - y0);
        let a = 2.0 * y0 - 2.0 * y1 + m0 + m1;
        let b = 3.0 * (y1 - y0) - 2.0 * m0 - m1;
        ((a * t + b) * t + m0) * t + y0
    }

    /// Rewrite the sample written `age` writes ago (1: the last one): the Chorus puts its
    /// feedback into the sample it has just written, the Flanger into its centre. Nothing
    /// happens for an age that reads silence (never written since the line was emptied).
    #[inline]
    pub fn set_back(&mut self, age: usize, x: f32) {
        if age == 0 || age > self.filled {
            return;
        }
        let i = if self.pos >= age {
            self.pos - age
        } else {
            self.pos + self.len - age
        };
        self.buf[i] = x;
    }

    /// The [`SINC_TAPS`] samples from `oldest` writes ago to `oldest − 31` (oldest first)
    /// weighed by `kernel` and summed: read straight from the buffer when they lie in one
    /// stretch of it, as they nearly always do (else, and while the line is filling, sample by
    /// sample, silence for what it doesn't hold). Needs `oldest` ≥ 32.
    #[inline]
    pub fn dot(&self, kernel: &[f32; SINC_TAPS], oldest: usize) -> f32 {
        if oldest <= self.filled && oldest <= self.len {
            let first = if self.pos >= oldest {
                self.pos - oldest
            } else {
                self.pos + self.len - oldest
            };
            if first + SINC_TAPS <= self.len {
                return dot(kernel, &self.buf[first..first + SINC_TAPS]);
            }
        }
        let mut sum = 0.0;
        for (k, w) in kernel.iter().enumerate() {
            sum += w * self.back(oldest - k);
        }
        sum
    }

    /// The signal `age` samples back (fractional, at least [`Sinc::MIN_AGE`]) by `sinc`'s
    /// windowed-sinc kernel.
    #[inline]
    pub fn sinc(&self, sinc: &Sinc, age: f64) -> f32 {
        let mut kernel = [0.0; SINC_TAPS];
        let oldest = sinc.kernel(age, &mut kernel);
        self.dot(&kernel, oldest)
    }
}

/// The taps of [`Sinc`]'s kernel.
pub const SINC_TAPS: usize = 32;

/// Whether the sinc kernel keeps the reference's own scaling (each tap divided by 1.08, as both
/// studies read it: a DC gain of 0.9676, -0.29 dB) or is scaled to pass DC at unity, which is
/// what an interpolator evidently means and what lets the Flanger's two taps null against the
/// dry sound where they meet. Unity (false) unless the reference renders show its -0.29 dB
/// (TD136): flip this to bring back the reference's arithmetic and null against them.
pub const SINC_REFERENCE_GAIN: bool = false;

/// The 32-tap windowed-sinc fractional read the Chorus (at Highest Quality, 4000
/// phases) and Flanger (240 phases) share, as the research notes on the chorus and flanger
/// describe it: for fraction `p/P` and tap `k`, with `t = k − p/P`,
/// `sinc(π(t − 15)/15 · 31/2.16) · (0.5001 − 0.4999·cos(π(30 − t)/15))`, its zeros 1.045 samples
/// apart (a cutoff about 4 % below Nyquist); a read blends the two phases either side of its
/// fraction linearly. Built once (in f64, kept in f32), never on the audio thread.
pub struct Sinc {
    phases: usize,
    /// `phases + 1` rows of [`SINC_TAPS`] weights, oldest tap first; the last row is a whole
    /// sample's shift, so a fraction just under 1 blends towards it.
    rows: Vec<[f32; SINC_TAPS]>,
}

impl Sinc {
    /// The youngest age a read can take: its newest tap is the sample just written.
    pub const MIN_AGE: f64 = 17.0;

    pub fn new(phases: usize) -> Self {
        let phases = phases.max(1);
        let rows = (0..=phases)
            .map(|p| {
                let frac = p as f64 / phases as f64;
                let mut row = [0.0f64; SINC_TAPS];
                for (k, w) in row.iter_mut().enumerate() {
                    let t = k as f64 - frac;
                    let x = std::f64::consts::PI * (t - 15.0) / 15.0 * 31.0 / 2.16;
                    let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
                    let window = 0.5001 - 0.4999 * (std::f64::consts::PI * (30.0 - t) / 15.0).cos();
                    *w = sinc * window;
                }
                let scale = if SINC_REFERENCE_GAIN {
                    1.0 / 1.08
                } else {
                    1.0 / row.iter().sum::<f64>()
                };
                row.map(|w| (w * scale) as f32)
            })
            .collect();
        Self { phases, rows }
    }

    pub fn phases(&self) -> usize {
        self.phases
    }

    /// The weights reading `age` back (at least [`Sinc::MIN_AGE`]) into `kernel`, oldest tap
    /// first, and the age of that oldest tap: the whole age at or above `age` is tap 15, the
    /// fraction towards the newer sample picks the phases.
    #[inline]
    pub fn kernel(&self, age: f64, kernel: &mut [f32; SINC_TAPS]) -> usize {
        let age = age.max(Self::MIN_AGE);
        let whole = age.ceil();
        let scaled = (whole - age) * self.phases as f64;
        let q = (scaled as usize).min(self.phases - 1);
        let a = (scaled - q as f64) as f32;
        let (lo, hi) = (&self.rows[q], &self.rows[q + 1]);
        for k in 0..SINC_TAPS {
            kernel[k] = lo[k] + a * (hi[k] - lo[k]);
        }
        whole as usize + 15
    }
}

/// `Σ a·b` over 32 values, in eight running sums the compiler can keep in vector registers.
#[inline]
fn dot(a: &[f32; SINC_TAPS], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; 8];
    for (x, y) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        for i in 0..8 {
            acc[i] += x[i] * y[i];
        }
    }
    acc.iter().sum()
}

/// A value moving in a straight line to a target over a number of samples, landing on it
/// exactly; read a sample at a time, so it moves the same at any block size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ramp {
    value: f32,
    target: f32,
    step: f32,
    left: usize,
}

impl Ramp {
    pub fn new(value: f32) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
            left: 0,
        }
    }

    /// Head for `target`, there `samples` from now (0: at once).
    pub fn to(&mut self, target: f32, samples: usize) {
        self.target = target;
        self.left = samples;
        if samples == 0 {
            self.value = target;
        } else {
            self.step = (target - self.value) / samples as f32;
        }
    }

    /// At the target now.
    pub fn snap(&mut self) {
        self.value = self.target;
        self.left = 0;
    }

    /// This sample's value; then one step on.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        let v = self.value;
        if self.left > 0 {
            self.left -= 1;
            self.value = if self.left == 0 {
                self.target
            } else {
                self.value + self.step
            };
        }
        v
    }

    /// The value now, without stepping.
    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn moving(&self) -> bool {
        self.left > 0
    }
}

/// A crossfade between two sets of settings, forwards: when the settings change, the output
/// moves from what the old set makes to what the new one makes in a straight line over a fixed
/// length, then the new set is the one in use. A change during a fade waits for it to end (the
/// latest change wins), as the reference's Delay and Echo queue theirs. The reference's own fade runs
/// backwards (it starts at the new set, fades back towards the old and snaps to the new): ours
/// is the one it evidently meant.
#[derive(Debug, Clone, Copy)]
pub struct Crossfade<T> {
    current: T,
    next: T,
    pending: Option<T>,
    length: usize,
    /// Samples of the fade still to come (0: none running).
    left: usize,
}

impl<T: Copy + PartialEq> Crossfade<T> {
    /// In use: `set`; fades take `length` samples (at least 1).
    pub fn new(set: T, length: usize) -> Self {
        Self {
            current: set,
            next: set,
            pending: None,
            length: length.max(1),
            left: 0,
        }
    }

    /// Move to `set`: a fade starts now, or when the running one ends.
    pub fn change(&mut self, set: T) {
        if self.left == 0 {
            if set != self.current {
                self.next = set;
                self.left = self.length;
            }
        } else if set == self.next {
            self.pending = None;
        } else {
            self.pending = Some(set);
        }
    }

    /// The set the last change asked for, at once, without a fade.
    pub fn snap(&mut self) {
        if let Some(p) = self.pending.take() {
            self.next = p;
        }
        if self.left > 0 {
            self.current = self.next;
            self.left = 0;
        }
    }

    /// The set in use, and the one faded to with this sample's weight on it (`None` when no fade
    /// runs); then one sample on. The weight rises 1/L, 2/L … 1 over a fade of L samples.
    #[inline]
    pub fn step(&mut self) -> (T, Option<(T, f32)>) {
        if self.left == 0 {
            return (self.current, None);
        }
        let (from, to) = (self.current, self.next);
        let weight = (self.length - self.left + 1) as f32 / self.length as f32;
        self.left -= 1;
        if self.left == 0 {
            self.current = self.next;
            if let Some(p) = self.pending.take() {
                if p != self.current {
                    self.next = p;
                    self.left = self.length;
                }
            }
        }
        (from, Some((to, weight)))
    }

    /// The set in use now (the old one while a fade runs).
    pub fn current(&self) -> T {
        self.current
    }

    /// The set the latest change asked for.
    pub fn target(&self) -> T {
        self.pending.unwrap_or(if self.left > 0 {
            self.next
        } else {
            self.current
        })
    }

    pub fn fading(&self) -> bool {
        self.left > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_reads_back_by_age_and_forgets_when_emptied() {
        let mut line = Line::new(8);
        for x in 1..=5 {
            line.write(x as f32);
        }
        assert_eq!((line.back(1), line.back(5), line.back(6)), (5.0, 1.0, 0.0));
        // Wrapped: the oldest in use is 8 back.
        for x in 6..=12 {
            line.write(x as f32);
        }
        assert_eq!((line.back(1), line.back(8), line.back(9)), (12.0, 5.0, 0.0));
        line.clear();
        assert_eq!(line.back(1), 0.0);
        line.write(7.0);
        assert_eq!((line.back(1), line.back(2)), (7.0, 0.0));
        // A shorter length in use: it wraps there.
        line.reset(3);
        for x in 1..=4 {
            line.write(x as f32);
        }
        assert_eq!((line.back(1), line.back(3), line.back(4)), (4.0, 2.0, 0.0));
    }

    #[test]
    fn the_cubic_passes_through_the_samples_and_is_the_common_form() {
        let mut line = Line::new(64);
        for t in 0..40 {
            line.write(((t * t) % 7) as f32 - 3.0);
        }
        // Through every sample at whole ages.
        for age in 3..30 {
            assert_eq!(line.cubic(age as f64), line.back(age));
        }
        // Between them, the form worked by hand: age 10.25 lies between ages 11 and 10, 0.75 of
        // the way from 11 (y0) to 10 (y1); the outer points are ages 12 and 9.
        let (ym, y0, y1, y2) = (line.back(12), line.back(11), line.back(10), line.back(9));
        let mu = 0.75f32;
        let a = y2 - y1 - ym + y0;
        let want = a * mu * mu * mu + (ym - y0 - a) * mu * mu + (y1 - ym) * mu + y0;
        assert!((line.cubic(10.25) - want).abs() < 1e-6);
        // A constant reads as itself anywhere.
        let mut flat = Line::new(16);
        (0..16).for_each(|_| flat.write(0.5));
        assert_eq!(flat.cubic(6.3), 0.5);
    }

    #[test]
    fn the_hermite_passes_through_the_samples_with_catmull_rom_tangents() {
        let mut line = Line::new(64);
        for t in 0..40 {
            line.write(((t * t) % 7) as f32 - 3.0);
        }
        for age in 2..30 {
            assert_eq!(line.hermite(age as f64), line.back(age));
        }
        // Age 10.25: 0.75 of the way from age 11 (y0) to age 10 (y1).
        let (ym, y0, y1, y2) = (line.back(12), line.back(11), line.back(10), line.back(9));
        let t = 0.75f32;
        let (m0, m1) = ((y1 - ym) / 2.0, (y2 - y0) / 2.0);
        let want = (2.0 * t.powi(3) - 3.0 * t * t + 1.0) * y0
            + (t.powi(3) - 2.0 * t * t + t) * m0
            + (-2.0 * t.powi(3) + 3.0 * t * t) * y1
            + (t.powi(3) - t * t) * m1;
        assert!((line.hermite(10.25) - want).abs() < 1e-5);
        // A straight line reads exactly (the tangents are its slope).
        let mut ramp = Line::new(16);
        (0..16).for_each(|i| ramp.write(i as f32));
        assert!((ramp.hermite(5.5) - 10.5).abs() < 1e-6);
    }

    #[test]
    fn a_sample_is_rewritten_in_place() {
        let mut line = Line::new(8);
        (1..=5).for_each(|x| line.write(x as f32));
        line.set_back(1, 50.0);
        line.set_back(4, 20.0);
        // Beyond what was written: nothing.
        line.set_back(6, 9.0);
        assert_eq!(
            (1..=6).map(|a| line.back(a)).collect::<Vec<_>>(),
            [50.0, 4.0, 3.0, 20.0, 1.0, 0.0]
        );
    }

    #[test]
    fn the_sinc_reads_a_band_limited_signal_between_its_samples() {
        for phases in [240, 4000] {
            let sinc = Sinc::new(phases);
            // Unity at DC in every phase.
            if !SINC_REFERENCE_GAIN {
                for row in &sinc.rows {
                    let sum: f64 = row.iter().map(|w| f64::from(*w)).sum();
                    assert!((sum - 1.0).abs() < 1e-6, "{sum}");
                }
            }
            // A 1 kHz sine at 48 kHz read anywhere between its samples, against the sine
            // itself: within 5e-4 (its passband ripple, and the 32nd tap past the window's
            // centre that tilts its phase by 1.6e-4 rad at 1 kHz), on both paths through the
            // buffer (a stretch straight through and one across the wrap).
            let w = 2.0 * std::f64::consts::PI * 1000.0 / 48_000.0;
            let mut line = Line::new(200);
            let n = 1000;
            for i in 0..n {
                line.write((i as f64 * w).sin() as f32);
            }
            for k in 0..400 {
                let age = 17.0 + k as f64 * 0.37;
                let want = ((n as f64 - age) * w).sin();
                let got = f64::from(line.sinc(&sinc, age));
                assert!(
                    (got - want).abs() < 5e-4,
                    "{phases}: age {age}: {got} {want}"
                );
            }
            // A whole age reads (nearly) the sample: the kernel low-passes a little.
            assert!((line.sinc(&sinc, 40.0) - line.back(40)).abs() < 5e-4);
        }
    }

    #[test]
    fn a_ramp_lands_on_its_target() {
        let mut r = Ramp::new(1.0);
        r.to(0.0, 4);
        let v: Vec<f32> = (0..6).map(|_| r.tick()).collect();
        assert_eq!(v, [1.0, 0.75, 0.5, 0.25, 0.0, 0.0]);
        r.to(2.0, 0);
        assert_eq!(r.tick(), 2.0);
    }

    #[test]
    fn a_crossfade_runs_forwards_and_queues_the_latest_change() {
        let mut x = Crossfade::new(0u8, 4);
        assert_eq!(x.step(), (0, None));
        x.change(1);
        x.change(2);
        x.change(3);
        let mut seen = Vec::new();
        for _ in 0..9 {
            seen.push(x.step());
        }
        assert_eq!(
            seen,
            [
                (0, Some((1, 0.25))),
                (0, Some((1, 0.5))),
                (0, Some((1, 0.75))),
                (0, Some((1, 1.0))),
                (1, Some((3, 0.25))),
                (1, Some((3, 0.5))),
                (1, Some((3, 0.75))),
                (1, Some((3, 1.0))),
                (3, None),
            ]
        );
        x.change(4);
        x.snap();
        assert_eq!((x.step(), x.target()), ((4, None), 4));
    }
}
