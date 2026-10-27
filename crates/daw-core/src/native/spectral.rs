//! The spectral framing Noise Reduction and Hiss Reduction share (dev plan 11.4), and the
//! noise print both learn.
//!
//! [`OverlapSave`]: frames of N samples every hop H = 2X (X = ⌊N/P⌋, P the
//! Precision Factor), weighed by a 4-term Blackman–Harris window going in that is *divided
//! back out* after the effect has changed the spectrum (overlap-save, not overlap-add). Of each
//! frame only 3X samples around its centre are used: the first X crossfade (raised cosine)
//! with the last frame's tail, the next X play as they are, the last X become the next tail.
//!
//! - The delay is exactly N samples.
//! - Frames fall on the effect's own count of samples, never on the caller's blocks, so the
//!   output is the same at any block size; [`OverlapSave::reset`] (from `NativeEffect::start`)
//!   restarts the count, so playback and export cut the stream in the same places.
//! - Both channels share one complex FFT (left real, right imaginary), and are
//!   pulled apart into their own half spectra for the effect, then put back together.
//! - Everything is allocated for the largest size when made; a new FFT size or precision
//!   re-cuts the buffers on the audio thread without allocating.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f64::consts::PI;
use std::sync::Arc;

/// The FFT sizes the effects offer, as powers of two.
pub const MIN_BITS: u32 = 6;
pub const MAX_BITS: u32 = 14;
pub const MAX_SIZE: usize = 1 << MAX_BITS;

/// An FFT Size parameter (log2 of the size) as the editor shows it.
pub fn format_size(bits: f64) -> String {
    format!(
        "{}",
        1usize << (bits.round() as u32).clamp(MIN_BITS, MAX_BITS)
    )
}

/// The Noise Reduction window at `k` of `n`: a 4-term Blackman–Harris shifted by one
/// sample (its second term over n − 1), 0 at the start and exactly 1 in the middle, mirrored;
/// rounded to f32.
pub fn window(k: usize, n: usize) -> f64 {
    let k = if k > n / 2 { n - k } else { k };
    if k == 0 {
        return 0.0;
    }
    if k == n / 2 {
        return 1.0;
    }
    let j = (k - 1) as f64;
    let n = n as f64;
    let w = 0.35875 - 0.48829 * (2.0 * PI * j / n).cos()
        + 0.14128 * (4.0 * PI * j / (n - 1.0)).cos()
        - 0.01168 * (6.0 * PI * j / n).cos();
    f64::from(w as f32)
}

/// A forward and an inverse FFT plan.
type Pair = (Arc<dyn Fft<f64>>, Arc<dyn Fft<f64>>);

/// FFT plans for every offered size, made once off the audio thread.
pub struct Plans {
    plans: Vec<(usize, Pair)>,
    scratch_len: usize,
}

impl Plans {
    pub fn new() -> Self {
        let mut planner = FftPlanner::new();
        let plans: Vec<_> = (MIN_BITS..=MAX_BITS)
            .map(|b| {
                let n = 1usize << b;
                (
                    n,
                    (planner.plan_fft_forward(n), planner.plan_fft_inverse(n)),
                )
            })
            .collect();
        let scratch_len = plans
            .iter()
            .map(|(_, (f, i))| f.get_inplace_scratch_len().max(i.get_inplace_scratch_len()))
            .max()
            .unwrap_or(0);
        Self { plans, scratch_len }
    }

    /// The forward and inverse FFT of `size`.
    pub fn of(&self, size: usize) -> Option<&Pair> {
        self.plans.iter().find(|p| p.0 == size).map(|p| &p.1)
    }

    pub fn scratch_len(&self) -> usize {
        self.scratch_len
    }
}

impl Default for Plans {
    fn default() -> Self {
        Self::new()
    }
}

/// Split `work` (the FFT of left + i·right, `size` long) into the two channels' half spectra,
/// bins `0..=size/2`: L[k] = (X[k] + conj X[−k]) / 2, R[k] = (X[k] − conj X[−k]) / 2i.
pub fn split(work: &[Complex<f64>], left: &mut [Complex<f64>], right: &mut [Complex<f64>]) {
    let size = work.len();
    for k in 0..=size / 2 {
        let x = work[k];
        let y = work[(size - k) % size].conj();
        left[k] = (x + y) * 0.5;
        let d = (x - y) * 0.5;
        right[k] = Complex::new(d.im, -d.re);
    }
}

/// The inverse of [`split`]: Y[k] = L[k] + i·R[k], Y[−k] = conj L[k] + i·conj R[k].
pub fn join(left: &[Complex<f64>], right: &[Complex<f64>], work: &mut [Complex<f64>]) {
    let size = work.len();
    let half = size / 2;
    for k in 0..=half {
        let (l, r) = (left[k], right[k]);
        work[k] = l + Complex::new(-r.im, r.re);
        if k != 0 && k != half {
            work[size - k] = l.conj() + Complex::new(r.im, r.re);
        }
    }
}

/// The two channels' half spectra of one frame (bins `0..=size/2`), for the effect to change.
pub struct Frame<'a> {
    pub left: &'a mut [Complex<f64>],
    pub right: &'a mut [Complex<f64>],
}

pub struct OverlapSave {
    plans: Plans,
    size: usize,
    /// X = ⌊N/P⌋: the crossfade, the plain stretch and the tail are X samples each.
    x: usize,
    /// Where a frame's plain stretch starts: trunc(⌊(P − 1)N/P⌋ / 2).
    centre: usize,
    window: Vec<f64>,
    /// 1/(N·w), the window divided out with the inverse FFT's scale; 0 where w is tiny.
    inverse: Vec<f64>,
    /// The raised-cosine crossfade, in and out, X long, in f32.
    fade_in: Vec<f64>,
    fade_out: Vec<f64>,
    /// The last N input samples of each channel, a ring indexed by the sample count.
    input: [Vec<f64>; 2],
    /// Finished output by sample time, a ring of 2N, read N samples late.
    output: [Vec<f64>; 2],
    /// Each channel's tail from the last frame, to crossfade into the next.
    tail: [Vec<f64>; 2],
    work: Vec<Complex<f64>>,
    left: Vec<Complex<f64>>,
    right: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
    /// Samples since the start.
    t: usize,
    /// The frames made so far, and the sample count at which the next is due.
    frames: usize,
    due: usize,
}

impl OverlapSave {
    /// Frames of `size` (a power of two from 2^[`MIN_BITS`] to 2^[`MAX_BITS`]) with precision
    /// factor `precision` (3 or more).
    pub fn new(size: usize, precision: usize) -> Self {
        let plans = Plans::new();
        let zero = Complex::new(0.0, 0.0);
        let mut os = Self {
            scratch: vec![zero; plans.scratch_len()],
            plans,
            size: 0,
            x: 0,
            centre: 0,
            window: Vec::with_capacity(MAX_SIZE),
            inverse: Vec::with_capacity(MAX_SIZE),
            fade_in: Vec::with_capacity(MAX_SIZE),
            fade_out: Vec::with_capacity(MAX_SIZE),
            input: [vec![0.0; MAX_SIZE], vec![0.0; MAX_SIZE]],
            output: [vec![0.0; 2 * MAX_SIZE], vec![0.0; 2 * MAX_SIZE]],
            tail: [vec![0.0; MAX_SIZE], vec![0.0; MAX_SIZE]],
            work: vec![zero; MAX_SIZE],
            left: vec![zero; MAX_SIZE / 2 + 1],
            right: vec![zero; MAX_SIZE / 2 + 1],
            t: 0,
            frames: 0,
            due: 0,
        };
        os.resize(size, precision);
        os
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Frames of `size` with precision factor `precision`, starting afresh. No allocation.
    pub fn resize(&mut self, size: usize, precision: usize) {
        let size = size.clamp(1 << MIN_BITS, MAX_SIZE).next_power_of_two();
        let p = precision.max(3);
        self.size = size;
        self.x = size / p;
        self.centre = ((p - 1) * size / p) / 2;
        self.window.clear();
        self.window.extend((0..size).map(|k| window(k, size)));
        self.inverse.clear();
        let n = size as f64;
        self.inverse.extend(self.window.iter().map(|&w| {
            if w <= 1e-7 {
                0.0
            } else {
                f64::from(((1.0 / n) / w) as f32)
            }
        }));
        let x = self.x as f64;
        let fade = |sign: f64, i: usize| {
            f64::from(((1.0 + sign * (PI * i as f64 / x).cos()) / 2.0) as f32)
        };
        self.fade_in.clear();
        self.fade_in.extend((0..self.x).map(|i| fade(-1.0, i)));
        self.fade_out.clear();
        self.fade_out.extend((0..self.x).map(|i| fade(1.0, i)));
        self.reset();
    }

    /// Start afresh: no input, nothing to play, the count at zero.
    pub fn reset(&mut self) {
        for c in 0..2 {
            self.input[c].fill(0.0);
            self.output[c].fill(0.0);
            self.tail[c].fill(0.0);
        }
        self.t = 0;
        self.frames = 0;
        // The first frame is due once N − centre samples are in (the centre's worth of zeros
        // before the start: the ring starts pre-filled).
        self.due = self.size - self.centre;
    }

    /// Run samples `from..to` of `left` and `right` through in place; each frame's spectra go
    /// to `effect` as they are made.
    pub fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        from: usize,
        to: usize,
        effect: &mut impl FnMut(Frame<'_>),
    ) {
        let (n, ring) = (self.size, 2 * self.size);
        for k in from..to {
            let at = self.t % n;
            self.input[0][at] = f64::from(left[k]);
            self.input[1][at] = f64::from(right[k]);
            self.t += 1;
            if self.t == self.due {
                self.frame(effect);
                self.frames += 1;
                self.due += 2 * self.x;
            }
            // What came in N samples ago, finished by now.
            if self.t > n {
                let slot = (self.t - 1 - n) % ring;
                left[k] = self.output[0][slot] as f32;
                right[k] = self.output[1][slot] as f32;
                self.output[0][slot] = 0.0;
                self.output[1][slot] = 0.0;
            } else {
                left[k] = 0.0;
                right[k] = 0.0;
            }
        }
    }

    fn frame(&mut self, effect: &mut impl FnMut(Frame<'_>)) {
        let (n, x, o) = (self.size, self.x, self.centre);
        let half = n / 2;
        let Some((forward, inverse)) = self.plans.of(n) else {
            return;
        };
        // The last N samples, oldest first: the ring from the next slot to be written.
        let oldest = self.t % n;
        let work = &mut self.work[..n];
        for (i, w) in work.iter_mut().enumerate() {
            let at = (oldest + i) % n;
            let g = self.window[i];
            *w = Complex::new(self.input[0][at] * g, self.input[1][at] * g);
        }
        forward.process_with_scratch(work, &mut self.scratch);
        split(work, &mut self.left[..=half], &mut self.right[..=half]);
        effect(Frame {
            left: &mut self.left[..=half],
            right: &mut self.right[..=half],
        });
        join(&self.left[..=half], &self.right[..=half], work);
        inverse.process_with_scratch(work, &mut self.scratch);
        // Frame j's plain stretch starts at sample time j·2X.
        let start = self.frames * 2 * x;
        let ring = 2 * n;
        for c in 0..2 {
            let y = |i: usize| {
                let v = if c == 0 { work[i].re } else { work[i].im };
                v * self.inverse[i]
            };
            if self.frames > 0 {
                for i in 0..x {
                    let v = self.fade_in[i] * y(o - x + i) + self.fade_out[i] * self.tail[c][i];
                    self.output[c][(start - x + i) % ring] = v;
                }
            }
            for i in 0..x {
                self.output[c][(start + i) % ring] = y(o + i);
                self.tail[c][i] = y(o + x + i);
            }
        }
    }
}

/// Room around a [`smooth`]er's bins, so its neighbours past either end read zero.
pub const PAD: usize = 4;

/// The noise reducers' magnitude smoother, bin `k` of state `s` (padded by [`PAD`] each side)
/// taking magnitude `a`: blended with its neighbours (this frame's on the left, already
/// updated; last frame's on the right), then followed down at `decay` (the Spectral Decay
/// Rate, 0..1) and up at once but for `1 − attack` of the old value. Returns the new value.
pub fn smooth(s: &mut [f64], k: usize, a: f64, decay: f64, attack: f64) -> f64 {
    let at = k + PAD;
    let blend = 0.4 * a + 0.2 * (s[at - 1] + s[at + 1]) + 0.1 * (s[at - 2] + s[at + 2]);
    s[at] = if blend <= s[at] {
        decay * s[at] + (1.0 - decay) * blend
    } else {
        (1.0 - attack) * s[at] + attack * blend
    };
    s[at]
}

/// The magnitude a bin reads in the noise reducers' scale: |X|·4/N of the windowed frame's
/// unscaled FFT (a full-scale sine centred on a bin reads about 0.72).
pub fn magnitude(bin: Complex<f64>, size: usize) -> f64 {
    bin.norm() * 4.0 / size as f64
}

/// One windowed frame of each channel of `audio`, `size` samples from `start`, as half
/// spectra (bins `0..=size/2`), for measuring a print on the main thread.
pub fn spectrum_at(
    audio: &[Vec<f32>],
    start: usize,
    size: usize,
    fft: &Arc<dyn Fft<f64>>,
    work: &mut Vec<Complex<f64>>,
    out: &mut [Vec<Complex<f64>>],
) {
    for (c, channel) in audio.iter().enumerate() {
        work.clear();
        work.extend((0..size).map(|i| {
            let s = channel.get(start + i).copied().unwrap_or(0.0);
            Complex::new(f64::from(s) * window(i, size), 0.0)
        }));
        fft.process(work);
        out[c].clear();
        out[c].extend_from_slice(&work[..=size / 2]);
    }
}

/// A print's layout, shared by the effects that learn one so it can be saved, read at another
/// FFT size or rate, and drawn: five header numbers (the rate, the FFT size, the channels, the
/// frames measured and the statistics per bin), then for each channel each statistic's
/// `size / 2 + 1` bins, magnitudes in [`magnitude`]'s scale. Statistic 0 is the bin's typical
/// level, which the editor draws.
pub struct Print<'a> {
    pub rate: f64,
    pub size: usize,
    pub channels: usize,
    pub frames: usize,
    pub stats: usize,
    data: &'a [f32],
}

const HEADER: usize = 5;

impl<'a> Print<'a> {
    /// Read `print`; `None` when its numbers don't add up.
    pub fn read(print: &'a [f32]) -> Option<Self> {
        let header = print.get(..HEADER)?;
        let p = Self {
            rate: f64::from(header[0]),
            size: header[1] as usize,
            channels: header[2] as usize,
            frames: header[3] as usize,
            stats: header[4] as usize,
            data: &print[HEADER..],
        };
        let ok = p.rate > 0.0
            && p.size >= 16
            && p.size.is_power_of_two()
            && p.channels > 0
            && p.stats > 0
            && p.data.len() == p.channels * p.stats * p.bins();
        ok.then_some(p)
    }

    /// A new print's numbers: the header, then `stats` (by channel, then statistic, each
    /// `size / 2 + 1` bins).
    pub fn write(rate: f64, size: usize, frames: usize, stats: &[Vec<Vec<f64>>]) -> Vec<f32> {
        let per = stats.first().map_or(0, Vec::len);
        let mut out = vec![
            rate as f32,
            size as f32,
            stats.len() as f32,
            frames as f32,
            per as f32,
        ];
        for channel in stats {
            for stat in channel {
                out.extend(stat.iter().map(|&v| v as f32));
            }
        }
        out
    }

    pub fn bins(&self) -> usize {
        self.size / 2 + 1
    }

    /// One channel's statistic, a value per bin (a mono print serves every channel).
    pub fn stat(&self, channel: usize, stat: usize) -> &'a [f32] {
        let channel = channel.min(self.channels - 1);
        let at = (channel * self.stats + stat) * self.bins();
        &self.data[at..at + self.bins()]
    }

    /// Statistic `stat` of `channel` for bin `k` of an FFT of `size` at `rate`: the print's
    /// own bin when they match, else read between its bins at the bin's frequency and scaled
    /// as steady noise scales with the FFT size (√(its size / `size`) in this scale).
    pub fn for_bin(&self, channel: usize, stat: usize, k: usize, size: usize, rate: f64) -> f64 {
        let values = self.stat(channel, stat);
        if size == self.size && rate == self.rate {
            return f64::from(values[k.min(values.len() - 1)]);
        }
        let hz = k as f64 * rate / size as f64;
        let bin = (hz * self.size as f64 / self.rate).clamp(0.0, (self.bins() - 1) as f64);
        let below = bin.floor() as usize;
        let above = (below + 1).min(self.bins() - 1);
        let t = bin - below as f64;
        let v = f64::from(values[below]) * (1.0 - t) + f64::from(values[above]) * t;
        v * (self.size as f64 / size as f64).sqrt()
    }

    /// Statistic 0 in dB at `hz`, the channels averaged in power: the editor's curve.
    pub fn level_db(&self, hz: f64) -> f64 {
        let k = (hz * self.size as f64 / self.rate).round() as usize;
        let power: f64 = (0..self.channels)
            .map(|c| {
                self.for_bin(c, 0, k.min(self.bins() - 1), self.size, self.rate)
                    .powi(2)
            })
            .sum::<f64>()
            / self.channels as f64;
        10.0 * power.max(1e-30).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(block: usize, size: usize, precision: usize) -> (Vec<f32>, Vec<f32>) {
        let n = 40_000;
        let mut l: Vec<f32> = (0..n).map(|i| ((i as f32) * 0.031).sin() * 0.5).collect();
        let mut r: Vec<f32> = (0..n).map(|i| ((i as f32) * 0.17).cos() * 0.25).collect();
        let mut os = OverlapSave::new(size, precision);
        let mut at = 0;
        while at < n {
            let to = (at + block).min(n);
            os.process(&mut l, &mut r, at, to, &mut |_| {});
            at = to;
        }
        (l, r)
    }

    #[test]
    fn the_window_peaks_at_one_in_the_middle_and_is_symmetric() {
        let n = 4096;
        assert_eq!(window(0, n), 0.0);
        assert_eq!(window(n / 2, n), 1.0);
        assert_eq!(window(10, n), window(n - 10, n));
        // Shifted by a sample: w[1] is the Blackman–Harris start, 6e-5.
        assert!((window(1, n) - 6e-5).abs() < 1e-6, "{}", window(1, n));
    }

    #[test]
    fn untouched_frames_come_back_as_the_input_delayed_by_n() {
        for (size, precision) in [(4096, 7), (1024, 3), (2048, 5), (256, 20)] {
            let (l, r) = run(512, size, precision);
            for i in (size + 2 * size)..40_000 {
                let want_l = (((i - size) as f32) * 0.031).sin() * 0.5;
                let want_r = (((i - size) as f32) * 0.17).cos() * 0.25;
                assert!(
                    (l[i] - want_l).abs() < 1e-5,
                    "{size}/{precision} left at {i}: {} vs {want_l}",
                    l[i]
                );
                assert!(
                    (r[i] - want_r).abs() < 1e-5,
                    "{size}/{precision} right at {i}"
                );
            }
        }
    }

    #[test]
    fn the_same_at_any_block_size() {
        assert_eq!(run(512, 4096, 7), run(77, 4096, 7));
    }

    #[test]
    fn a_print_reads_back_and_rescales() {
        let stats = vec![vec![vec![0.5; 9], vec![0.25; 9]]];
        let print = Print::write(48_000.0, 16, 12, &stats);
        let p = Print::read(&print).unwrap();
        assert_eq!((p.channels, p.stats, p.bins(), p.frames), (1, 2, 9, 12));
        assert_eq!(p.for_bin(1, 1, 3, 16, 48_000.0), 0.25);
        // Twice the FFT size: steady noise reads 1/√2 as much per bin.
        assert!((p.for_bin(0, 0, 4, 32, 48_000.0) - 0.5 / 2f64.sqrt()).abs() < 1e-9);
        assert!(Print::read(&print[..print.len() - 1]).is_none());
    }
}
