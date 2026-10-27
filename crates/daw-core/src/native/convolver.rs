//! The shared convolver (dev plan 11.6): a stereo impulse response applied to a stereo stream by
//! partitioned FFT convolution, for the Convolution Reverb (and the Space Reverb next).
//!
//! **Latency: a fixed [`LATENCY`] (256 samples) at any rate and any host block size.** The
//! impulse is cut into partitions convolved by uniformly partitioned overlap-save in the
//! frequency domain (a delay line of input spectra, each multiplied by its partition's spectrum
//! and summed), in two stages, each uniform within itself:
//!
//! - the **head**: partitions of 256 samples (FFTs of 512), covering the impulse's first
//!   `2·T − 256` samples. Its output for each 256 input samples is ready the moment they are in,
//!   so it is heard one partition later: that is the latency.
//! - the **tail**: partitions of `T` samples (1024 to 16384, FFTs of `2·T`) for the rest, chosen
//!   per impulse to make the whole cheapest ([`Plan::choose`]). A tail block's spectrum is
//!   taken when its `T` samples are in, its products are summed over the following `T` samples
//!   and its inverse FFT is made at the end of them: just in time, because the tail starts
//!   `2·T − 256` samples into the impulse.
//!
//! A single uniform stage of 256 would cost about `length/256` complex products per sample and
//! channel (over 1100 for a 6 s impulse at 48 kHz, and a 20 s impulse at 192 kHz could never run
//! live); with the tail it is a few tens to a hundred. A direct-form head for zero latency would
//! cost 256 products per sample by itself, more than everything else, so it is not done
//! (the reference's own is zero latency: TD129).
//!
//! **Bounded work per block.** Within a partition the products not waiting on its own input
//! (the head's partitions after the first, every one of the tail's) are spread evenly over the
//! samples as they arrive, so a 64-sample block does its share and the FFTs; the order of every
//! sum is fixed by the partition index, never by where host blocks fall, so the output is
//! bit-identical at any host block size.
//!
//! **Two channels in one FFT.** Left is the real part and right the imaginary part of one
//! complex FFT (rustfft, f32, as `spectral.rs` packs them); each channel's half spectrum is
//! split out of it, multiplied by its own partition, and the two are packed back for one
//! inverse. Spectra are stored as separate real and imaginary arrays so the products vectorise.
//! The impulse's gain (and the inverse FFT's 1/N) is folded into its spectra.
//!
//! **Real-time:** everything (spectra, the delay lines, the FFT plans and their scratch) is
//! allocated by [`Convolver::new`], off the audio thread; [`Convolver::process`] allocates,
//! locks and frees nothing. [`Convolver::reset`] clears the small state (input windows, sums,
//! the output ring) and empties the spectra's delay line logically (a count of blocks seen), so
//! a transport start costs no pass over megabytes.
//!
//! **Draining.** A convolver replaced by a new impulse goes on playing the tail of the input it
//! had, fed silence ([`Convolver::quiet`]): products of blocks wholly after the switch are
//! skipped, and [`Convolver::drained`] says when its last sample is out, so a swap leaves no gap
//! and no cut tail.

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// The convolver's delay (samples): one head partition.
pub const LATENCY: usize = 256;
/// The head's partition.
const HEAD: usize = LATENCY;
/// The tail partitions considered.
const TAILS: [usize; 5] = [1024, 2048, 4096, 8192, 16_384];

/// How an impulse is partitioned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Head partitions (of [`LATENCY`] samples).
    pub head: usize,
    /// The tail's partition size, 0 for none.
    pub tail_size: usize,
    /// Tail partitions.
    pub tail: usize,
}

impl Plan {
    /// The cheapest plan for an impulse `len` samples long: head partitions alone, or a head
    /// up to `2·T − 256` and partitions of `T` after it. Cost per sample in flops, roughly:
    /// 16 per partition (a complex product for each of two channels, about one bin per sample)
    /// and 20·log2(FFT size) per stage (a forward and an inverse FFT of both channels).
    pub fn choose(len: usize) -> Self {
        let len = len.max(1);
        let fft = |size: usize| 20.0 * ((2 * size) as f64).log2();
        let head_only = Self {
            head: len.div_ceil(HEAD),
            tail_size: 0,
            tail: 0,
        };
        let mut best = (16.0 * head_only.head as f64 + fft(HEAD), head_only);
        for size in TAILS {
            let start = 2 * size - HEAD;
            if len <= start {
                continue;
            }
            let plan = Self {
                head: start / HEAD,
                tail_size: size,
                tail: (len - start).div_ceil(size),
            };
            let cost = 16.0 * (plan.head + plan.tail) as f64 + fft(HEAD) + fft(size);
            if cost < best.0 {
                best = (cost, plan);
            }
        }
        best.1
    }

    /// Where the tail starts in the impulse.
    fn tail_start(&self) -> usize {
        self.head * HEAD
    }
}

/// One uniformly partitioned stage: partitions of `size` samples from `start` in the impulse.
struct Stage {
    size: usize,
    /// Bins of a half spectrum: `size + 1`.
    bins: usize,
    parts: usize,
    /// Each partition's spectrum per channel, `[(part·2 + channel)·bins + bin]`.
    h_re: Vec<f32>,
    h_im: Vec<f32>,
    /// The input's spectra, the same layout, block `b` in slot `b % parts`.
    x_re: Vec<f32>,
    x_im: Vec<f32>,
    /// The sum being made, per channel.
    acc_re: Vec<f32>,
    acc_im: Vec<f32>,
    /// The last `2·size` input samples per channel: the previous block, then the one filling.
    window: [Vec<f32>; 2],
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
    work: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Blocks transformed since the reset.
    blocks: u64,
    /// Products done towards the sum being made.
    done: usize,
    /// The tail's sum holds a block's products to turn into output at the next boundary.
    armed: bool,
}

impl Stage {
    /// Partitions of `size` from `start` of `impulse` (two channels), `parts` of them, scaled
    /// by `scale`.
    fn new(
        planner: &mut FftPlanner<f32>,
        impulse: [&[f32]; 2],
        start: usize,
        size: usize,
        parts: usize,
        scale: f32,
    ) -> Self {
        let n = 2 * size;
        let bins = size + 1;
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let scratch_len = fwd
            .get_inplace_scratch_len()
            .max(inv.get_inplace_scratch_len());
        let mut stage = Self {
            size,
            bins,
            parts,
            h_re: vec![0.0; parts * 2 * bins],
            h_im: vec![0.0; parts * 2 * bins],
            x_re: vec![0.0; parts * 2 * bins],
            x_im: vec![0.0; parts * 2 * bins],
            acc_re: vec![0.0; 2 * bins],
            acc_im: vec![0.0; 2 * bins],
            window: [vec![0.0; n], vec![0.0; n]],
            fwd,
            inv,
            work: vec![Complex::default(); n],
            scratch: vec![Complex::default(); scratch_len],
            blocks: 0,
            done: 0,
            armed: false,
        };
        // The inverse FFT is unscaled: its 1/N rides in the spectra with the impulse's gain.
        let gain = scale / n as f32;
        for p in 0..parts {
            let from = start + p * size;
            for (i, z) in stage.work.iter_mut().enumerate() {
                let at = |c: usize| {
                    if i < size {
                        impulse[c].get(from + i).copied().unwrap_or(0.0) * gain
                    } else {
                        0.0
                    }
                };
                *z = Complex::new(at(0), at(1));
            }
            stage
                .fwd
                .process_with_scratch(&mut stage.work, &mut stage.scratch);
            let (re, im) = (&mut stage.h_re, &mut stage.h_im);
            split(&stage.work, size, p * 2 * bins, re, im);
        }
        stage
    }

    fn reset(&mut self) {
        for w in &mut self.window {
            w.fill(0.0);
        }
        self.acc_re.fill(0.0);
        self.acc_im.fill(0.0);
        self.blocks = 0;
        self.done = 0;
        self.armed = false;
    }

    /// Take the next `src.len()` input samples of channel `c`, `filled` into the block.
    fn push(&mut self, c: usize, filled: usize, src: Option<&[f32]>, len: usize) {
        let to = &mut self.window[c][self.size + filled..self.size + filled + len];
        match src {
            Some(src) => to.copy_from_slice(src),
            None => to.fill(0.0),
        }
    }

    /// The block just filled: its spectrum into the delay line, and the window moves on.
    /// Skipped (the slot is never read) for a block wholly after `quiet`.
    fn transform(&mut self, quiet: Option<u64>) {
        let block = self.blocks;
        self.blocks += 1;
        if !self.is_silent(block, quiet) {
            let [l, r] = &self.window;
            for (z, (a, b)) in self.work.iter_mut().zip(l.iter().zip(r)) {
                *z = Complex::new(*a, *b);
            }
            self.fwd
                .process_with_scratch(&mut self.work, &mut self.scratch);
            let slot = (block % self.parts as u64) as usize;
            split(
                &self.work,
                self.size,
                slot * 2 * self.bins,
                &mut self.x_re,
                &mut self.x_im,
            );
        }
        for w in &mut self.window {
            w.copy_within(self.size.., 0);
        }
    }

    /// Whether block `block`'s window (the samples `(block − 1)·size .. (block + 1)·size`) lies
    /// wholly at or after `quiet`, where the input is silence.
    fn is_silent(&self, block: u64, quiet: Option<u64>) -> bool {
        quiet.is_some_and(|q| match block {
            0 => q == 0,
            b => (b - 1) * self.size as u64 >= q,
        })
    }

    /// Add partition `p`'s product with the input block it meets in output block `target`.
    fn mac(&mut self, p: usize, target: u64, quiet: Option<u64>) {
        let Some(block) = target.checked_sub(p as u64) else {
            return;
        };
        // Not seen since the reset (before it: silence), or silence after a switch.
        if block >= self.blocks || self.is_silent(block, quiet) {
            return;
        }
        let bins = self.bins;
        let slot = (block % self.parts as u64) as usize;
        for c in 0..2 {
            let h = (p * 2 + c) * bins;
            let x = (slot * 2 + c) * bins;
            let (hr, hi) = (&self.h_re[h..h + bins], &self.h_im[h..h + bins]);
            let (xr, xi) = (&self.x_re[x..x + bins], &self.x_im[x..x + bins]);
            let (ar, ai) = (
                &mut self.acc_re[c * bins..(c + 1) * bins],
                &mut self.acc_im[c * bins..(c + 1) * bins],
            );
            for k in 0..bins {
                ar[k] += hr[k] * xr[k] - hi[k] * xi[k];
                ai[k] += hr[k] * xi[k] + hi[k] * xr[k];
            }
        }
    }

    /// The sum into output: its valid half added to `ring` from `at`, and the sum cleared.
    fn inverse(&mut self, ring: &mut [Vec<f32>; 2], at: u64) {
        let (size, bins) = (self.size, self.bins);
        let n = 2 * size;
        let (lr, li) = (&self.acc_re[..bins], &self.acc_im[..bins]);
        let (rr, ri) = (&self.acc_re[bins..], &self.acc_im[bins..]);
        // Both channels back into one spectrum: Z = L + i·R, Z[n − k] = conj(L) + i·conj(R).
        for k in 0..bins {
            self.work[k] = Complex::new(lr[k] - ri[k], li[k] + rr[k]);
        }
        for k in 1..size {
            self.work[n - k] = Complex::new(lr[k] + ri[k], rr[k] - li[k]);
        }
        self.inv
            .process_with_scratch(&mut self.work, &mut self.scratch);
        let mask = ring[0].len() - 1;
        let [l, r] = ring;
        for (i, z) in self.work[size..].iter().enumerate() {
            let j = (at as usize + i) & mask;
            l[j] += z.re;
            r[j] += z.im;
        }
        self.acc_re.fill(0.0);
        self.acc_im.fill(0.0);
    }
}

/// One complex FFT of left + i·right (`2·size` points) into each channel's half spectrum, at
/// `offset` (left) and `offset + size + 1` (right) of `re` and `im`.
fn split(z: &[Complex<f32>], size: usize, offset: usize, re: &mut [f32], im: &mut [f32]) {
    let n = 2 * size;
    let bins = size + 1;
    for k in 0..bins {
        let a = z[k];
        let b = z[(n - k) % n].conj();
        // L = (Z[k] + conj Z[n − k]) / 2, R = (Z[k] − conj Z[n − k]) / 2i.
        re[offset + k] = 0.5 * (a.re + b.re);
        im[offset + k] = 0.5 * (a.im + b.im);
        re[offset + bins + k] = 0.5 * (a.im - b.im);
        im[offset + bins + k] = -0.5 * (a.re - b.re);
    }
}

/// A stereo impulse ready to convolve, with every buffer it needs.
pub struct Convolver {
    plan: Plan,
    head: Stage,
    tail: Option<Stage>,
    /// What has been computed and not yet played, per channel, by time modulo its length.
    ring: [Vec<f32>; 2],
    /// Samples processed since the reset.
    time: u64,
    /// The impulse's length (samples).
    len: usize,
    /// From here on the input is silence (a convolver draining after a switch).
    quiet: Option<u64>,
}

impl Convolver {
    /// Prepare `impulse` (one channel, played on both, or two), scaled by `scale`. Off the audio
    /// thread: this allocates everything [`Convolver::process`] uses.
    pub fn new(impulse: &[Vec<f32>], scale: f32) -> Self {
        let len = impulse.iter().map(Vec::len).max().unwrap_or(0);
        Self::with_plan(impulse, scale, Plan::choose(len))
    }

    /// [`Convolver::new`] with a given plan (the tests force a tail on short impulses).
    pub fn with_plan(impulse: &[Vec<f32>], scale: f32, plan: Plan) -> Self {
        let len = impulse.iter().map(Vec::len).max().unwrap_or(0);
        let empty: &[f32] = &[];
        let channels = [
            impulse.first().map_or(empty, Vec::as_slice),
            impulse
                .get(1)
                .or(impulse.first())
                .map_or(empty, Vec::as_slice),
        ];
        let mut planner = FftPlanner::new();
        let head = Stage::new(&mut planner, channels, 0, HEAD, plan.head.max(1), scale);
        let tail = (plan.tail > 0 && plan.tail_size > 0).then(|| {
            Stage::new(
                &mut planner,
                channels,
                plan.tail_start(),
                plan.tail_size,
                plan.tail,
                scale,
            )
        });
        let ring_len = (2 * plan.tail_size.max(HEAD)).next_power_of_two();
        Self {
            plan,
            head,
            tail,
            ring: [vec![0.0; ring_len], vec![0.0; ring_len]],
            time: 0,
            len,
            quiet: None,
        }
    }

    pub fn plan(&self) -> Plan {
        self.plan
    }

    /// The impulse's length in samples.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Bytes held for the spectra and delay lines, for the editor and the measurements.
    pub fn bytes(&self) -> usize {
        let stage = |s: &Stage| (s.h_re.len() * 4 + s.window[0].len() * 2) * 4;
        stage(&self.head) + self.tail.as_ref().map_or(0, stage) + self.ring[0].len() * 8
    }

    /// Start again from silence (the transport started). No allocation; the spectra's delay
    /// line is emptied by forgetting its blocks.
    pub fn reset(&mut self) {
        self.head.reset();
        if let Some(tail) = &mut self.tail {
            tail.reset();
        }
        for r in &mut self.ring {
            r.fill(0.0);
        }
        self.time = 0;
        self.quiet = None;
    }

    /// From now on the input is silence: what it had rings on, and [`Convolver::drained`] says
    /// when it is all out.
    pub fn quiet(&mut self) {
        self.quiet.get_or_insert(self.time);
    }

    /// Silent for good (quieted before its first sample: a transport start while draining).
    pub fn silence(&mut self) {
        self.reset();
        self.quiet = Some(0);
    }

    /// Quieted, and everything it had to play has been played.
    pub fn drained(&self) -> bool {
        self.quiet
            .is_some_and(|q| q == 0 || self.time >= q + (self.len + LATENCY) as u64)
    }

    /// Convolve `input` (left and right; `None`: silence) into `output` (overwritten), each
    /// `output[0].len()` samples, delayed by [`LATENCY`]. Audio thread: no allocation.
    pub fn process(&mut self, input: Option<[&[f32]; 2]>, output: [&mut [f32]; 2]) {
        let frames = output[0].len().min(output[1].len());
        let mask = self.ring[0].len() - 1;
        let [out_l, out_r] = output;
        let mut i = 0;
        while i < frames {
            let filled = (self.time % HEAD as u64) as usize;
            let n = (HEAD - filled).min(frames - i);
            // What is due now was computed at an earlier boundary.
            for k in 0..n {
                let j = (self.time as usize + k) & mask;
                out_l[i + k] = std::mem::take(&mut self.ring[0][j]);
                out_r[i + k] = std::mem::take(&mut self.ring[1][j]);
            }
            for c in 0..2 {
                let src = input.map(|inp| &inp[c][i..i + n]);
                self.head.push(c, filled, src, n);
                if let Some(tail) = &mut self.tail {
                    let tail_filled = (self.time % tail.size as u64) as usize;
                    tail.push(c, tail_filled, src, n);
                }
            }
            self.time += n as u64;
            i += n;
            let quiet = self.quiet;
            // The head: partitions 1.. meet blocks already in, spread over the block's samples;
            // partition 0 waits for the block itself.
            let head = &mut self.head;
            let target = head.blocks;
            let in_block = filled + n;
            let due = ((head.parts - 1) * in_block).div_ceil(HEAD);
            while head.done < due {
                head.done += 1;
                head.mac(head.done, target, quiet);
            }
            // The tail: all its partitions for the block before, spread over this one.
            if let Some(tail) = &mut self.tail {
                if tail.armed {
                    let in_block = match (self.time % tail.size as u64) as usize {
                        0 => tail.size,
                        f => f,
                    };
                    let due = (tail.parts * in_block).div_ceil(tail.size);
                    let target = tail.blocks - 1;
                    while tail.done < due {
                        tail.mac(tail.done, target, quiet);
                        tail.done += 1;
                    }
                }
            }
            if self.time.is_multiple_of(HEAD as u64) {
                let head = &mut self.head;
                head.transform(quiet);
                head.mac(0, target, quiet);
                head.inverse(&mut self.ring, self.time);
                head.done = 0;
            }
            if let Some(tail) = &mut self.tail {
                if self.time.is_multiple_of(tail.size as u64) {
                    if tail.armed {
                        tail.inverse(&mut self.ring, self.time);
                    }
                    tail.transform(quiet);
                    tail.armed = true;
                    tail.done = 0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic noise in ±1.
    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                ((x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
            })
            .collect()
    }

    /// Run `input` (two channels) through `conv` in blocks of `block`, plus the latency and
    /// `extra` samples more of silence; the output with the latency taken off.
    fn run(
        conv: &mut Convolver,
        input: &[Vec<f32>; 2],
        block: usize,
        extra: usize,
    ) -> [Vec<f32>; 2] {
        let total = input[0].len() + extra + LATENCY;
        let mut out = [vec![0.0; total], vec![0.0; total]];
        let mut pos = 0;
        let mut inp = [vec![0.0f32; block], vec![0.0f32; block]];
        while pos < total {
            let n = block.min(total - pos);
            for (to, from) in inp.iter_mut().zip(input) {
                for (k, x) in to[..n].iter_mut().enumerate() {
                    *x = from.get(pos + k).copied().unwrap_or(0.0);
                }
            }
            let [l, r] = &mut out;
            conv.process(
                Some([&inp[0][..n], &inp[1][..n]]),
                [&mut l[pos..pos + n], &mut r[pos..pos + n]],
            );
            pos += n;
        }
        for o in &mut out {
            o.drain(..LATENCY);
        }
        out
    }

    /// The straight sum, in f64.
    fn direct(x: &[f32], h: &[f32], len: usize) -> Vec<f64> {
        (0..len)
            .map(|n| {
                let lo = n.saturating_sub(h.len() - 1);
                (lo..=n.min(x.len().saturating_sub(1)))
                    .filter(|&m| m < x.len())
                    .map(|m| f64::from(x[m]) * f64::from(h[n - m]))
                    .sum()
            })
            .collect()
    }

    /// The difference's RMS relative to the reference's, in dB.
    fn null_db(ours: &[f32], reference: &[f64]) -> f64 {
        let (mut d, mut r) = (0.0, 0.0);
        for (a, b) in ours.iter().zip(reference) {
            d += (f64::from(*a) - b).powi(2);
            r += b * b;
        }
        10.0 * (d / r).log10()
    }

    #[test]
    fn plans_take_a_tail_for_long_impulses() {
        assert_eq!(
            Plan::choose(1000),
            Plan {
                head: 4,
                tail_size: 0,
                tail: 0
            }
        );
        // 6 s at 48 kHz: a tail of 4096 or 8192, far fewer partitions than 1125 of 256.
        let six = Plan::choose(288_000);
        assert!(six.tail_size >= 4096, "{six:?}");
        assert!(six.head + six.tail < 120, "{six:?}");
        assert_eq!(six.head * LATENCY, 2 * six.tail_size - LATENCY);
        assert!(six.head * LATENCY + six.tail * six.tail_size >= 288_000);
        // 20 s at 192 kHz: still a few hundred.
        let long = Plan::choose(3_840_000);
        assert_eq!(long.tail_size, 16_384, "{long:?}");
        assert!(long.head + long.tail < 400, "{long:?}");
    }

    #[test]
    fn a_unit_impulse_gives_the_input_delayed_by_the_latency() {
        let input = [noise(10_000, 1), noise(10_000, 2)];
        for impulse in [vec![vec![1.0f32]], vec![vec![1.0f32; 1], vec![1.0f32; 1]]] {
            let mut conv = Convolver::new(&impulse, 1.0);
            for block in [64, 77, 512] {
                conv.reset();
                let out = run(&mut conv, &input, block, 0);
                for c in 0..2 {
                    let reference: Vec<f64> = input[c].iter().map(|&x| f64::from(x)).collect();
                    let db = null_db(&out[c][..10_000], &reference);
                    println!("unit impulse, channel {c}, blocks of {block}: {db:.1} dB");
                    assert!(db < -100.0, "channel {c}, blocks of {block}: {db:.1} dB");
                }
            }
        }
    }

    #[test]
    fn a_random_impulse_gives_the_direct_convolution_at_any_block_size() {
        // 9000 samples: the head and three tail partitions of 2048 (forced), so both stages
        // and their hand-over at 3840 are in it; a different impulse on each side.
        let h = [
            noise(9000, 7)
                .iter()
                .enumerate()
                .map(|(i, x)| x * (-(i as f32) / 3000.0).exp())
                .collect::<Vec<_>>(),
            noise(9000, 8)
                .iter()
                .enumerate()
                .map(|(i, x)| x * (-(i as f32) / 2000.0).exp())
                .collect::<Vec<_>>(),
        ];
        let input = [noise(30_000, 3), noise(30_000, 4)];
        let plan = Plan {
            head: (2 * 2048 - LATENCY) / LATENCY,
            tail_size: 2048,
            tail: 3,
        };
        let mut conv = Convolver::with_plan(&h, 0.25, plan);
        let len = input[0].len() + 9000;
        let reference: Vec<Vec<f64>> = (0..2)
            .map(|c| {
                direct(&input[c], &h[c], len)
                    .into_iter()
                    .map(|v| v * 0.25)
                    .collect()
            })
            .collect();
        let mut first: Option<[Vec<f32>; 2]> = None;
        for block in [512, 77, 1, 4096] {
            conv.reset();
            let out = run(&mut conv, &input, block, 9000);
            for c in 0..2 {
                let db = null_db(&out[c][..len], &reference[c]);
                println!("random impulse, channel {c}, blocks of {block}: {db:.1} dB");
                assert!(db < -100.0, "channel {c}, blocks of {block}: {db:.1} dB");
            }
            // And the same samples whatever the block size.
            match &first {
                None => first = Some(out),
                Some(f) => assert!(f == &out, "blocks of {block} differ from 512"),
            }
        }
        // Its own plan gives the same within rounding.
        let mut own = Convolver::new(&h, 0.25);
        let out = run(&mut own, &input, 333, 9000);
        assert!(null_db(&out[0][..len], &reference[0]) < -100.0);
    }

    #[test]
    fn a_quieted_convolver_plays_its_tail_and_drains() {
        let h = vec![noise(5000, 5)];
        let input = [noise(4000, 6), noise(4000, 9)];
        let mut conv = Convolver::with_plan(
            &h,
            1.0,
            Plan {
                head: 7,
                tail_size: 1024,
                tail: 4,
            },
        );
        // Input for 4000 samples, then quieted and fed nothing.
        let total = 4000 + 5000 + LATENCY;
        let mut out = [vec![0.0; total], vec![0.0; total]];
        {
            let [l, r] = &mut out;
            conv.process(
                Some([&input[0], &input[1]]),
                [&mut l[..4000], &mut r[..4000]],
            );
            conv.quiet();
            assert!(!conv.drained());
            let mut at = 4000;
            while at < total {
                let n = 100.min(total - at);
                conv.process(None, [&mut l[at..at + n], &mut r[at..at + n]]);
                at += n;
            }
        }
        assert!(conv.drained());
        for (c, o) in out.iter().enumerate() {
            let reference = direct(&input[c], &h[0], total - LATENCY);
            let db = null_db(&o[LATENCY..], &reference);
            assert!(db < -100.0, "channel {c}: {db:.1} dB");
        }
    }

    /// Release timing (run with `cargo test -p daw-core --release -- --ignored
    /// convolver_cost --nocapture`): the cost per second of audio and the worst 64-sample
    /// block for 2 s and 6 s stereo impulses at 48 kHz (and 20 s at 192 kHz, the longest
    /// asked for), and the time to prepare them. The worst block is also given as the 99.9th
    /// percentile, since a single one can be the OS taking the thread away.
    #[test]
    #[ignore]
    fn convolver_cost() {
        for (rate, seconds) in [(48_000usize, 2usize), (48_000, 6), (192_000, 20)] {
            let len = rate * seconds;
            let h = [noise(len, 11), noise(len, 12)];
            let started = std::time::Instant::now();
            let mut conv = Convolver::new(&h, 1e-3);
            let prepare = started.elapsed();
            let input = [noise(rate, 13), noise(rate, 14)];
            let mut out = [vec![0.0f32; 64], vec![0.0f32; 64]];
            let mut block = |b: usize, conv: &mut Convolver| {
                let at = (b * 64) % (rate - 64);
                let [l, r] = &mut out;
                let t = std::time::Instant::now();
                conv.process(
                    Some([&input[0][at..at + 64], &input[1][at..at + 64]]),
                    [&mut l[..], &mut r[..]],
                );
                t.elapsed()
            };
            // A second to warm up, then ten seconds of 64-sample blocks.
            for b in 0..rate / 64 {
                block(b, &mut conv);
            }
            let started = std::time::Instant::now();
            let mut times: Vec<std::time::Duration> =
                (0..10 * rate / 64).map(|b| block(b, &mut conv)).collect();
            let per_second = started.elapsed().as_secs_f64() / 10.0;
            times.sort_unstable();
            let worst = times[times.len() - 1];
            let p999 = times[times.len() * 999 / 1000];
            println!(
                "{seconds} s stereo impulse at {} kHz: plan {:?}, {:.1} MB, prepared in {:.1} ms; \
                 {:.2} ms per second of audio ({:.2} % of a core), 64-sample blocks: worst {:.3} \
                 ms, 99.9th percentile {:.3} ms (a block lasts {:.3} ms)",
                rate / 1000,
                conv.plan(),
                conv.bytes() as f64 / 1e6,
                prepare.as_secs_f64() * 1e3,
                per_second * 1e3,
                per_second * 100.0,
                worst.as_secs_f64() * 1e3,
                p999.as_secs_f64() * 1e3,
                64_000.0 / rate as f64
            );
        }
    }
}
