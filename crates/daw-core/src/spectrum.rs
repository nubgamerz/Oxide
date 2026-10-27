//! Spectra of a file for the Waveform view's spectral display (dev plan 10.3) and for agents.
//!
//! Levels are dB of a sine's amplitude: a full-scale sine reads 0 dB, so a magnitude is
//! `20·log10(2·|X| / Σ window)` (Hann windows). Frames that run past either end of the audio
//! are zero-padded.
//!
//! A rendered view has one FFT frame per column, centred on that column's sample: its cost is
//! the view's width times the FFT size, whatever the file's length. Zoomed far out (many FFTs'
//! worth of samples per column) a column shows the one frame at its centre and skips the
//! rest.

use crate::analysis::hann;
use rustfft::{num_complex::Complex, FftPlanner};

/// The display's lowest frequency.
pub const MIN_HZ: f64 = 20.0;
/// Quieter than this is the colour map's floor whatever its range.
const SILENT_DB: f32 = -200.0;

/// An FFT size for a zoom of `spp` samples per point: about four columns' worth, from 1024
/// (fine time detail when zoomed in) to 8192 (fine frequency detail when zoomed out).
pub fn fft_size_for(spp: f64) -> usize {
    ((spp * 4.0).max(1.0) as usize)
        .next_power_of_two()
        .clamp(1024, 8192)
}

/// A spectrogram of a view: `db[row * width + column]`, row 0 the highest frequency (the
/// Nyquist frequency), the last row [`MIN_HZ`], on a log scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrogram {
    pub width: usize,
    pub height: usize,
    pub db: Vec<f32>,
    /// How many FFT frames it took (one per column).
    pub frames: usize,
}

/// The frequency at the middle of `row` of a `height`-row display, on the log scale.
pub fn row_hz(row: usize, height: usize, rate: u32) -> f64 {
    let nyquist = f64::from(rate) / 2.0;
    let t = 1.0 - (row as f64 + 0.5) / height as f64;
    MIN_HZ * (nyquist / MIN_HZ).powf(t)
}

/// The row of a `height`-row display showing `hz`.
pub fn hz_row(hz: f64, height: usize, rate: u32) -> f32 {
    let nyquist = f64::from(rate) / 2.0;
    let t = (hz.max(MIN_HZ) / MIN_HZ).ln() / (nyquist / MIN_HZ).ln();
    ((1.0 - t) * height as f64) as f32
}

/// One Hann-windowed FFT frame of `samples` centred on `centre`, as amplitude in dB per bin
/// (`fft / 2 + 1` bins).
struct Frames {
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    norm: f32,
    buffer: Vec<Complex<f32>>,
}

impl Frames {
    fn new(size: usize) -> Self {
        let window = hann(size);
        let norm = 2.0 / window.iter().sum::<f32>();
        Self {
            fft: FftPlanner::new().plan_fft_forward(size),
            window,
            norm,
            buffer: vec![Complex::new(0.0, 0.0); size],
        }
    }

    /// The amplitudes (linear) of the frame centred on `centre`.
    fn amplitudes(&mut self, samples: &[f32], centre: i64) -> impl Iterator<Item = f32> + '_ {
        let size = self.window.len() as i64;
        let first = centre - size / 2;
        for (k, slot) in self.buffer.iter_mut().enumerate() {
            let i = first + k as i64;
            let v = usize::try_from(i)
                .ok()
                .and_then(|i| samples.get(i))
                .copied()
                .unwrap_or(0.0);
            *slot = Complex::new(v * self.window[k], 0.0);
        }
        self.fft.process(&mut self.buffer);
        let norm = self.norm;
        self.buffer[..=self.window.len() / 2]
            .iter()
            .map(move |c| c.norm() * norm)
    }
}

fn db(amplitude: f32) -> f32 {
    if amplitude > 0.0 {
        (20.0 * amplitude.log10()).max(SILENT_DB)
    } else {
        SILENT_DB
    }
}

/// Render a view of `samples` (at `rate`): `width` columns starting at sample `start`, `spp`
/// samples apart, `height` rows of log frequency, with FFTs of `fft` points. Each row shows the
/// loudest bin in its band (the nearest bin where a band is narrower than one).
pub fn render(
    samples: &[f32],
    rate: u32,
    start: f64,
    spp: f64,
    width: usize,
    height: usize,
    fft: usize,
) -> Spectrogram {
    let mut frames = Frames::new(fft);
    let bin_hz = f64::from(rate) / fft as f64;
    let last_bin = fft / 2;
    // Each row's bins: from its lower edge to its upper edge, at least one.
    let rows: Vec<(usize, usize)> = (0..height)
        .map(|r| {
            let edge = |row: f64| {
                let nyquist = f64::from(rate) / 2.0;
                MIN_HZ * (nyquist / MIN_HZ).powf(1.0 - row / height as f64)
            };
            let (hi, lo) = (edge(r as f64), edge(r as f64 + 1.0));
            let (a, b) = (
                ((lo / bin_hz).round() as usize).min(last_bin),
                ((hi / bin_hz).round() as usize).min(last_bin),
            );
            (a, b.max(a))
        })
        .collect();
    let mut out = vec![SILENT_DB; width * height];
    let mut column = vec![0.0f32; last_bin + 1];
    for x in 0..width {
        let centre = (start + (x as f64 + 0.5) * spp) as i64;
        for (slot, a) in column.iter_mut().zip(frames.amplitudes(samples, centre)) {
            *slot = a;
        }
        for (r, &(a, b)) in rows.iter().enumerate() {
            let loudest = column[a..=b].iter().fold(0.0f32, |m, v| m.max(*v));
            out[r * width + x] = db(loudest);
        }
    }
    Spectrogram {
        width,
        height,
        db: out,
        frames: width,
    }
}

/// A band of a spectrum: its edges and its level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub low_hz: f64,
    pub high_hz: f64,
    pub db: f32,
}

/// The spectrum of `samples[start..end]` in `bands` log-spaced bands from [`MIN_HZ`] to the
/// Nyquist frequency: FFT frames half overlapping, their power averaged (linear), each band
/// the loudest bin's level in it. Returns the bands and how many frames were averaged.
pub fn bands(
    samples: &[f32],
    rate: u32,
    start: usize,
    end: usize,
    count: usize,
    fft: usize,
) -> (Vec<Band>, usize) {
    let mut frames = Frames::new(fft);
    let hop = fft / 2;
    let end = end.min(samples.len());
    let mut power = vec![0.0f64; fft / 2 + 1];
    let mut n = 0usize;
    let mut centre = start + hop;
    loop {
        for (p, a) in power
            .iter_mut()
            .zip(frames.amplitudes(samples, centre as i64))
        {
            *p += f64::from(a) * f64::from(a);
        }
        n += 1;
        centre += hop;
        if centre + hop > end {
            break;
        }
    }
    let bin_hz = f64::from(rate) / fft as f64;
    let nyquist = f64::from(rate) / 2.0;
    let count = count.max(1);
    let edge = |k: usize| MIN_HZ * (nyquist / MIN_HZ).powf(k as f64 / count as f64);
    // A bin belongs to the band its centre frequency falls in; a band too narrow for any
    // (low frequencies, small FFTs) shows the bin nearest its middle.
    let bands = (0..count)
        .map(|k| {
            let (lo, hi) = (edge(k), edge(k + 1));
            let a = (lo / bin_hz).ceil() as usize;
            let b = ((hi / bin_hz).ceil() as usize).min(power.len());
            let loudest = if a < b {
                power[a..b].iter().fold(0.0f64, |m, p| m.max(*p))
            } else {
                let mid = (((lo * hi).sqrt() / bin_hz).round() as usize).min(power.len() - 1);
                power[mid]
            } / n as f64;
            Band {
                low_hz: lo,
                high_hz: hi,
                db: db(loudest.sqrt() as f32),
            }
        })
        .collect();
    (bands, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const FFT: usize = 4096;

    /// A sine on an FFT bin (85 × 48000 / 4096 ≈ 996 Hz), so its level is exact.
    fn tone(seconds: f64, amplitude: f32) -> (Vec<f32>, f64) {
        let hz = 85.0 * f64::from(RATE) / FFT as f64;
        let n = (seconds * f64::from(RATE)) as usize;
        let samples = (0..n)
            .map(|i| {
                amplitude
                    * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(RATE)).sin() as f32
            })
            .collect();
        (samples, hz)
    }

    #[test]
    fn a_sine_shows_on_its_row_at_its_level() {
        let (samples, hz) = tone(1.0, 0.5);
        let (width, height) = (40, 200);
        let s = render(&samples, RATE, 0.0, 1000.0, width, height, FFT);
        let column = width / 2;
        let (row, level) = (0..height)
            .map(|r| (r, s.db[r * width + column]))
            .fold((0, f32::MIN), |best, x| if x.1 > best.1 { x } else { best });
        let shown = row_hz(row, height, RATE);
        assert!(
            (shown / hz - 1.0).abs() < 0.05,
            "the brightest row shows {shown:.0} Hz, the tone is {hz:.0} Hz"
        );
        assert!((level + 6.02).abs() < 0.5, "{level} dB, not -6.02");
        assert!((hz_row(hz, height, RATE) - row as f32).abs() <= 1.0);
    }

    #[test]
    fn bands_peak_where_the_tone_is() {
        let (samples, hz) = tone(1.0, 0.5);
        let (bands, frames) = bands(&samples, RATE, 0, samples.len(), 32, FFT);
        assert!(frames > 10);
        let loudest = bands
            .iter()
            .fold(bands[0], |m, b| if b.db > m.db { *b } else { m });
        assert!(loudest.low_hz <= hz && hz < loudest.high_hz, "{loudest:?}");
        assert!((loudest.db + 6.02).abs() < 0.5, "{} dB", loudest.db);
    }

    #[test]
    fn a_render_costs_its_width_not_the_files_length() {
        // A long file and a short one, the same view size: the same number of frames.
        let (short, _) = tone(1.0, 0.5);
        let long = vec![0.1f32; 48_000 * 600];
        let a = render(&short, RATE, 0.0, 40.0, 100, 50, FFT);
        let b = render(&long, RATE, 1e6, 5000.0, 100, 50, FFT);
        assert_eq!(a.frames, 100);
        assert_eq!(b.frames, 100);
        // Past the end: silence, not a panic.
        let past = render(&short, RATE, 1e7, 100.0, 10, 10, 1024);
        assert!(past.db.iter().all(|v| *v <= SILENT_DB));
    }

    #[test]
    fn the_fft_grows_with_the_zoom() {
        assert_eq!(fft_size_for(1.0), 1024);
        assert_eq!(fft_size_for(600.0), 4096);
        assert_eq!(fft_size_for(1e6), 8192);
    }
}
