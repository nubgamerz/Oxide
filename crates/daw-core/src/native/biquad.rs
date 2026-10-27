//! Second-order filter sections for the equalisers (dev plan 11.1): coefficients, their
//! response for the editor's curve, and a stereo filter that runs them.
//!
//! Coefficients are normalised (`a0 = 1`): `y = b0·x + b1·x₁ + b2·x₂ − a1·y₁ − a2·y₂`. The
//! filter runs transposed direct form II in double precision, which keeps low-frequency bands
//! quiet and stable at any setting. A section whose state has died away (all of it under
//! [`super::TINY64`], −600 dB) is zeroed: left alone, it decays into f64 denormals once the
//! input stops and can stick there (the De-esser's split cost up to 1.6 times as much on silence
//! as on music, dev plan 18.1).

use super::flush_all;
use crate::graph::CHANNELS;
use std::f64::consts::{PI, SQRT_2};

/// One section's coefficients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coefficients {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coefficients {
    /// Passes everything unchanged.
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// From un-normalised coefficients (`a0` any non-zero value).
    pub fn new(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// The gain at `hz` (sample rate `rate`), linear.
    pub fn magnitude(&self, hz: f64, rate: f64) -> f64 {
        let ((nr, ni), (dr, di)) = self.parts(hz, rate);
        ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt()
    }

    /// The response at `hz` (sample rate `rate`) as a complex number (real, imaginary), for
    /// curves that add filtered signals rather than chain them (the Hum Remover's hum alone).
    pub fn response(&self, hz: f64, rate: f64) -> (f64, f64) {
        let ((nr, ni), (dr, di)) = self.parts(hz, rate);
        let d = dr * dr + di * di;
        ((nr * dr + ni * di) / d, (ni * dr - nr * di) / d)
    }

    /// The numerator and denominator at `hz`, each as (real, imaginary).
    fn parts(&self, hz: f64, rate: f64) -> ((f64, f64), (f64, f64)) {
        let w = 2.0 * PI * hz / rate;
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        // H(e^jw) = (b0 + b1 e^-jw + b2 e^-2jw) / (1 + a1 e^-jw + a2 e^-2jw)
        let (nr, ni) = (
            self.b0 + self.b1 * c1 + self.b2 * c2,
            -self.b1 * s1 - self.b2 * s2,
        );
        let (dr, di) = (
            1.0 + self.a1 * c1 + self.a2 * c2,
            -self.a1 * s1 - self.a2 * s2,
        );
        ((nr, ni), (dr, di))
    }
}

/// A run of sections on both channels, with their state.
#[derive(Debug, Clone)]
pub struct Filter<const N: usize> {
    pub sections: [Coefficients; N],
    /// Per channel and section: the two state values of transposed direct form II.
    state: [[[f64; 2]; N]; CHANNELS],
}

impl<const N: usize> Default for Filter<N> {
    fn default() -> Self {
        Self {
            sections: [Coefficients::IDENTITY; N],
            state: [[[0.0; 2]; N]; CHANNELS],
        }
    }
}

impl<const N: usize> Filter<N> {
    /// Filter samples `from..to` of each channel in place through every section.
    pub fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        for (buf, state) in io.iter_mut().zip(self.state.iter_mut()) {
            for s in &mut buf[from..to] {
                let mut x = f64::from(*s);
                for (c, z) in self.sections.iter().zip(state.iter_mut()) {
                    let y = c.b0 * x + z[0];
                    *z = flush_all([c.b1 * x - c.a1 * y + z[1], c.b2 * x - c.a2 * y]);
                    x = y;
                }
                *s = x as f32;
            }
        }
    }

    /// The whole run's gain at `hz`, in dB.
    pub fn response_db(&self, hz: f64, rate: f64) -> f64 {
        let gain: f64 = self
            .sections
            .iter()
            .map(|c| c.magnitude(hz, rate))
            .product();
        20.0 * gain.max(1e-12).log10()
    }
}

/// One section with its state on both channels, a sample at a time (transposed direct form
/// II, double), for effects that split the signal into bands.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub c: Coefficients,
    z: [[f64; 2]; CHANNELS],
}

impl Section {
    pub fn new(c: Coefficients) -> Self {
        Self {
            c,
            z: [[0.0; 2]; CHANNELS],
        }
    }

    /// Filter one sample of channel `ch`.
    pub fn tick(&mut self, ch: usize, x: f64) -> f64 {
        let (c, z) = (&self.c, &mut self.z[ch]);
        let y = c.b0 * x + z[0];
        *z = flush_all([c.b1 * x - c.a1 * y + z[1], c.b2 * x - c.a2 * y]);
        y
    }

    /// Whether its state is all zero (for tests of the flush).
    #[cfg(test)]
    pub fn is_silent(&self) -> bool {
        self.z == [[0.0; 2]; CHANNELS]
    }
}

/// A second-order Butterworth low- or high-pass at `f` Hz, prewarped there.
pub fn butterworth2(rate: f64, f: f64, high: bool) -> Coefficients {
    let wc = (PI * (f / rate).min(0.49)).tan();
    let zeta = SQRT_2;
    let (a0, a1, a2) = (
        1.0 + zeta * wc + wc * wc,
        2.0 * (wc * wc - 1.0),
        1.0 - zeta * wc + wc * wc,
    );
    if high {
        Coefficients::new(1.0, -2.0, 1.0, a0, a1, a2)
    } else {
        Coefficients::new(wc * wc, 2.0 * wc * wc, wc * wc, a0, a1, a2)
    }
}

/// A second-order allpass at `f` Hz with Butterworth damping: what a Linkwitz–Riley
/// low-pass and high-pass at `f` add up to, for keeping a band in phase with a split above it.
pub fn allpass2(rate: f64, f: f64) -> Coefficients {
    let wc = (PI * (f / rate).min(0.49)).tan();
    let (a0, a1, a2) = (
        1.0 + SQRT_2 * wc + wc * wc,
        2.0 * (wc * wc - 1.0),
        1.0 - SQRT_2 * wc + wc * wc,
    );
    Coefficients::new(a2, a1, a0, a0, a1, a2)
}

/// The gain of an amount in dB.
pub fn db_to_amplitude(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_passes_samples_unchanged() {
        let mut f = Filter::<3>::default();
        let input: Vec<f32> = (0..500).map(|i| (i as f32 * 0.3).sin()).collect();
        let mut io = vec![input.clone(), input.clone()];
        f.process(&mut io, 0, 500);
        assert_eq!(io[0], input);
        assert!((f.response_db(1000.0, 48_000.0)).abs() < 1e-12);
    }

    #[test]
    fn a_one_pole_style_section_has_the_response_it_measures() {
        // A simple low-pass: y = 0.5 x + 0.5 x1 (zero at Nyquist).
        let c = Coefficients::new(0.5, 0.5, 0.0, 1.0, 0.0, 0.0);
        assert!((c.magnitude(0.0, 48_000.0) - 1.0).abs() < 1e-12);
        assert!(c.magnitude(23_999.0, 48_000.0) < 1e-3);
        let mut f = Filter::<1> {
            sections: [c],
            ..Default::default()
        };
        // A sine at a quarter of the rate comes out at |H| = cos(pi/4).
        let n = 4000;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * i as f64 / 4.0).sin() as f32)
            .collect();
        let mut io = vec![input.clone(), input];
        f.process(&mut io, 0, n);
        let rms = |v: &[f32]| {
            (v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        let ratio = rms(&io[0][100..]) / (0.5f64).sqrt();
        assert!(
            (ratio - c.magnitude(12_000.0, 48_000.0)).abs() < 1e-3,
            "{ratio}"
        );
    }

    #[test]
    fn a_dying_section_ends_at_zero_not_in_the_denormals() {
        // A resonant low band rings on after the input stops; its double state reaches an
        // exact zero (flushed under 1e-30) instead of running down through f64's denormals.
        let c = Coefficients::new(0.001, 0.0, -0.001, 1.0, -1.998, 0.9985);
        let mut section = Section::new(c);
        let mut filter = Filter::<1> {
            sections: [c],
            ..Filter::default()
        };
        let mut io = vec![vec![0.0f32; 1000], vec![0.0f32; 1000]];
        io[0][0] = 1.0;
        io[1][0] = 1.0;
        for s in &io[0] {
            section.tick(0, f64::from(*s));
        }
        filter.process(&mut io, 0, 1000);
        let mut silence = vec![vec![0.0f32; 48_000]; CHANNELS];
        for _ in 0..10 {
            for _ in 0..48_000 {
                section.tick(0, 0.0);
            }
            silence.iter_mut().for_each(|c| c.fill(0.0));
            filter.process(&mut silence, 0, 48_000);
        }
        assert!(section.is_silent());
        assert_eq!(filter.state, [[[0.0; 2]; 1]; CHANNELS]);
    }
}
