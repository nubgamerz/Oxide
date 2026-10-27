//! Time-stretch and pitch-shift on real audio (dev plan 7.1): a recording (a guitar DI) through
//! `stretch::render` at several ratios and transpositions. Nothing is played; the renders go
//! to `out/stretch/` to listen to, and the recording is only read.
//!
//! For each render:
//! - **Length** must be exactly `round(length × ratio)`.
//! - **Level** (RMS over the whole file) within 1 dB of the original.
//! - **Onsets**: the transients found in the original (`analysis::detect_onsets`) must reappear
//!   at their stretched times. Each is paired with the nearest onset in the render; the median
//!   error must be under 20 ms, and nine in ten must be found within 50 ms. The worst and the
//!   sign are printed: a phase vocoder smears an attack a little early.
//! - **Pitch**: in sustained stretches between onsets, the strongest spectral peaks of the
//!   original are found again in the render at the stretched time, shifted by the
//!   transposition. The median error must be under 10 cents; the worst is printed.
//! - **Speed**: the realtime factor (seconds of audio rendered per second). Measured in a
//!   release build only: the C++ library is slow unoptimised. It must stay above 1×, as 7.2
//!   renders clips in the background.
//!
//! Prints `RESULT: PASS` or `FAIL:` lines. Set `STRETCH_ONSETS=1` to list every onset (ms /
//! strength), expected and found.
//!
//! cargo run --release -p daw-core --example stretch_check -- [<recording.wav>]

use anyhow::{bail, Result};
use daw_core::analysis::{detect_onsets, OnsetSettings};
use daw_core::stretch::{
    protect_transients, render, render_warped, stretched_length, ATTACK_AFTER_MS, ATTACK_BEFORE_MS,
};
use daw_core::wav;
use rustfft::{num_complex::Complex, FftPlanner};
use std::f64::consts::PI;
use std::path::Path;
use std::time::Instant;

/// (ratio, semitones) to render.
const CASES: [(f64, f64); 7] = [
    (0.5, 0.0),
    (0.8, 0.0),
    (1.25, 0.0),
    (2.0, 0.0),
    (1.0, 12.0),
    (1.0, -5.0),
    (1.25, 3.0),
];
const FFT: usize = 8192;
const PEAKS: usize = 6;

fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / signal.len().max(1) as f64).sqrt()
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(f64::NAN)
}

/// Spectral peaks of the `FFT` samples centred on `centre`: (frequency in Hz, log magnitude),
/// local maxima between 70 Hz and 5 kHz within 40 dB of the strongest, refined by parabolic
/// interpolation. `None` if the window doesn't fit.
fn peaks(signal: &[f32], centre: usize, rate: u32) -> Option<Vec<(f64, f64)>> {
    let start = centre.checked_sub(FFT / 2)?;
    let window = signal.get(start..start + FFT)?;
    let mut buffer: Vec<Complex<f64>> = window
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / FFT as f64).cos();
            Complex::new(f64::from(*s) * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(FFT).process(&mut buffer);
    let mags: Vec<f64> = buffer[..FFT / 2]
        .iter()
        .map(|c| c.norm().max(1e-12).ln())
        .collect();
    let bin = f64::from(rate) / FFT as f64;
    let (low, high) = ((70.0 / bin) as usize, (5000.0 / bin) as usize);
    let strongest = mags[low..high].iter().copied().fold(f64::MIN, f64::max);
    let floor = strongest - 40.0 / 20.0 * 10f64.ln();
    let mut found: Vec<(f64, f64)> = (low..high)
        .filter(|&k| mags[k] > floor && mags[k] > mags[k - 1] && mags[k] >= mags[k + 1])
        .map(|k| {
            let (a, b, c) = (mags[k - 1], mags[k], mags[k + 1]);
            let shift = 0.5 * (a - c) / (a - 2.0 * b + c);
            ((k as f64 + shift) * bin, b - 0.25 * (a - c) * shift)
        })
        .collect();
    found.sort_by(|x, y| y.1.total_cmp(&x.1));
    Some(found)
}

struct Measured {
    /// Attacks as clear as the original's (found by the original's own detector settings).
    clear: usize,
    onset_median_ms: f64,
    onset_worst_ms: f64,
    onsets_found: f64,
    onsets: usize,
    cents_median: f64,
    cents_worst: f64,
    peaks: usize,
}

/// Compare a render with the original: onsets and spectral peaks at the stretched times.
fn compare(original: &[f32], stretched: &[f32], rate: u32, ratio: f64, semitones: f64) -> Measured {
    let settings = OnsetSettings::new(0.5, 50.0, rate);
    let before = detect_onsets(original, settings);
    // Smeared attacks are found with a keener detector; how many stay as clear as in the
    // original is reported apart, as `clear`.
    let after = detect_onsets(stretched, OnsetSettings::new(1.0, 50.0, rate));
    let clear = detect_onsets(stretched, settings).len();
    let ms = |samples: f64| samples * 1000.0 / f64::from(rate);
    if std::env::var_os("STRETCH_ONSETS").is_some() {
        let show = |list: &[daw_core::analysis::Onset], scale: f64| {
            list.iter()
                .map(|o| format!("{:.0}/{:.2}", ms(o.position as f64 * scale), o.strength))
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!("    expected: {}", show(&before, ratio));
        println!("    found:    {}", show(&after, 1.0));
    }
    let mut errors: Vec<f64> = before
        .iter()
        .filter_map(|o| {
            let expected = o.position as f64 * ratio;
            after
                .iter()
                .map(|a| ms(a.position as f64 - expected))
                .min_by(|x, y| x.abs().total_cmp(&y.abs()))
        })
        .collect();
    let found = errors.iter().filter(|e| e.abs() <= 50.0).count();
    let onset_worst_ms = errors
        .iter()
        .copied()
        .max_by(|x, y| x.abs().total_cmp(&y.abs()))
        .unwrap_or(f64::NAN);
    let mut absolute: Vec<f64> = errors.iter().map(|e| e.abs()).collect();
    let onset_median_ms = median(&mut absolute);
    let signed_median = median(&mut errors);

    // Sustained windows: halfway between onsets at least 300 ms apart (and from the end).
    let mut marks: Vec<usize> = before.iter().map(|o| o.position).collect();
    marks.push(original.len());
    let gap = (0.3 * f64::from(rate)) as usize;
    let factor = 2f64.powf(semitones / 12.0);
    let mut cents = Vec::new();
    for pair in marks.windows(2) {
        if pair[1] - pair[0] < gap {
            continue;
        }
        let centre = (pair[0] + pair[1]) / 2;
        let (Some(ours), Some(theirs)) = (
            peaks(original, centre, rate),
            peaks(stretched, (centre as f64 * ratio) as usize, rate),
        ) else {
            continue;
        };
        for (f, _) in ours.iter().take(PEAKS) {
            let want = f * factor;
            if let Some(c) = theirs
                .iter()
                .map(|(g, _)| 1200.0 * (g / want).log2())
                .filter(|c| c.abs() < 50.0)
                .min_by(|x, y| x.abs().total_cmp(&y.abs()))
            {
                cents.push(c);
            }
        }
    }
    let cents_worst = cents
        .iter()
        .copied()
        .max_by(|x, y| x.abs().total_cmp(&y.abs()))
        .unwrap_or(f64::NAN);
    let mut absolute: Vec<f64> = cents.iter().map(|c| c.abs()).collect();
    println!(
        "    onsets: signed median {signed_median:+.1} ms, {clear} of {} as clear as the original's",
        before.len()
    );
    Measured {
        clear,
        onset_median_ms,
        onset_worst_ms,
        onsets_found: found as f64 / errors.len().max(1) as f64,
        onsets: before.len(),
        cents_median: median(&mut absolute),
        cents_worst,
        peaks: cents.len(),
    }
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("give the audio file (a WAV) as the first argument"))?;
    let audio = wav::read(Path::new(&path))?;
    let rate = audio.sample_rate;
    let original = &audio.channels[0];
    let seconds = original.len() as f64 / f64::from(rate);
    println!(
        "{path}: {} channel(s), {rate} Hz, {seconds:.2} s{}",
        audio.channels.len(),
        if cfg!(debug_assertions) {
            " (debug build: the realtime factor means nothing)"
        } else {
            ""
        }
    );
    let folder = Path::new("out/stretch");
    std::fs::create_dir_all(folder)?;
    let mut failures = Vec::new();
    // The plain rows' clear counts, for the kept-attack rows to beat.
    let mut kept_against: Vec<(f64, usize)> = Vec::new();

    for (ratio, semitones) in CASES {
        let label = format!("x{ratio} {semitones:+} st");
        let started = Instant::now();
        let out = render(&audio.channels, rate, ratio, semitones)?;
        let speed = seconds / started.elapsed().as_secs_f64();
        let name = format!("di_x{ratio}_{semitones:+}st.wav");
        wav::write_f32(
            &folder.join(&name),
            &wav::Audio {
                sample_rate: rate,
                channels: out.clone(),
            },
        )?;
        let stretched = &out[0];
        let level = 20.0 * (rms(stretched) / rms(original)).log10();
        let m = compare(original, stretched, rate, ratio, semitones);
        if semitones == 0.0 {
            kept_against.push((ratio, m.clear));
        }
        println!(
            "{label}: {name}, {speed:.0}x realtime, level {level:+.2} dB, onsets {}: median \
             {:.1} ms, worst {:+.1} ms, {:.0}% within 50 ms; {} peaks: median {:.1} cents, worst \
             {:+.1}",
            m.onsets,
            m.onset_median_ms,
            m.onset_worst_ms,
            m.onsets_found * 100.0,
            m.peaks,
            m.cents_median,
            m.cents_worst
        );
        let wanted = stretched_length(original.len(), ratio);
        if out.iter().any(|c| c.len() != wanted) {
            failures.push(format!("{label}: {} frames, not {wanted}", stretched.len()));
        }
        if level.abs() > 1.0 {
            failures.push(format!("{label}: level {level:+.2} dB"));
        }
        if m.onsets == 0 || m.onset_median_ms > 20.0 || m.onsets_found < 0.9 {
            failures.push(format!(
                "{label}: onsets median {:.1} ms, {:.0}% found",
                m.onset_median_ms,
                m.onsets_found * 100.0
            ));
        }
        if m.peaks == 0 || m.cents_median > 10.0 {
            failures.push(format!(
                "{label}: pitch median {:.1} cents over {} peaks",
                m.cents_median, m.peaks
            ));
        }
        if !cfg!(debug_assertions) && speed < 1.0 {
            failures.push(format!("{label}: {speed:.2}x realtime"));
        }
    }

    // Keeping attacks (TD75): each plays 1:1 and on time, the audio between takes the stretch.
    // Against the plain rows above: at least as many attacks as clear, and at 2x clearly more.
    let onsets: Vec<usize> = detect_onsets(original, OnsetSettings::new(0.5, 50.0, rate))
        .iter()
        .map(|o| o.position)
        .collect();
    let ms = |v: f64| (v * f64::from(rate) / 1000.0) as usize;
    for (ratio, plain_clear) in kept_against {
        let base = [(original.len(), stretched_length(original.len(), ratio))];
        let warp = protect_transients(&base, &onsets, ms(ATTACK_BEFORE_MS), ms(ATTACK_AFTER_MS));
        let out = render_warped(&audio.channels, rate, &warp, 0.0)?;
        let name = format!("di_x{ratio}_kept.wav");
        wav::write_f32(
            &folder.join(&name),
            &wav::Audio {
                sample_rate: rate,
                channels: out.clone(),
            },
        )?;
        let m = compare(original, &out[0], rate, ratio, 0.0);
        println!(
            "x{ratio} attacks kept: {name}, {} spans, {} of {} clear (plain {plain_clear}), onsets \
             median {:.1} ms, worst {:+.1} ms; pitch median {:.1} cents",
            warp.len(),
            m.clear,
            m.onsets,
            m.onset_median_ms,
            m.onset_worst_ms,
            m.cents_median
        );
        let label = format!("x{ratio} attacks kept");
        if m.clear < plain_clear {
            failures.push(format!(
                "{label}: {} clear, fewer than plain {plain_clear}",
                m.clear
            ));
        }
        if ratio == 2.0 && (m.clear < 8 || m.onset_median_ms > 6.0) {
            failures.push(format!(
                "{label}: {} clear, onsets median {:.1} ms (want >= 8, <= 6)",
                m.clear, m.onset_median_ms
            ));
        }
        if out[0].len() != base[0].1 || m.cents_median > 10.0 {
            failures.push(format!(
                "{label}: {} frames, {:.1} cents",
                out[0].len(),
                m.cents_median
            ));
        }
    }
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("stretch check failed")
    }
}
