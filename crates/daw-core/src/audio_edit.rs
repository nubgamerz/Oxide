//! Edits to a whole audio file, as the Waveform view makes them (dev plan 10.2): pure functions
//! from one file's channels to new ones. Ranges are the file's own samples. Edits that change
//! the length (delete, trim, insert silence, paste) apply to every channel; the others can
//! take a list of channels.

use crate::loudness::LoudnessMeter;
use crate::project::FadeShape;
use crate::spectral_edit::{self, SpectralBox, SpectralOp};
use anyhow::{bail, Result};

/// A range of samples, `start..end`.
pub type Range = (u64, u64);

/// Audio copied from a file, for pasting: its rate and channels.
#[derive(Debug, Clone, PartialEq)]
pub struct Clipboard {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

/// One edit. A `range` of `None` is the whole file; `channels` of `None` is every channel.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Delete {
        range: Range,
    },
    /// Keep only the range.
    Trim {
        range: Range,
    },
    InsertSilence {
        at: u64,
        length: u64,
    },
    Silence {
        range: Option<Range>,
        channels: Option<Vec<usize>>,
    },
    Amplify {
        range: Option<Range>,
        channels: Option<Vec<usize>>,
        db: f64,
    },
    /// Scale so the loudest sample reaches `peak_db` dBFS.
    NormalizePeak {
        range: Option<Range>,
        channels: Option<Vec<usize>>,
        peak_db: f64,
    },
    /// Scale so the integrated loudness (EBU R 128) reaches `lufs`.
    NormalizeLoudness {
        range: Option<Range>,
        channels: Option<Vec<usize>>,
        lufs: f64,
    },
    FadeIn {
        range: Range,
        channels: Option<Vec<usize>>,
        shape: FadeShape,
    },
    FadeOut {
        range: Range,
        channels: Option<Vec<usize>>,
        shape: FadeShape,
    },
    Reverse {
        range: Option<Range>,
        channels: Option<Vec<usize>>,
    },
    Invert {
        range: Option<Range>,
        channels: Option<Vec<usize>>,
    },
    /// Paste the clipboard at `at`, or over `range` (replacing it).
    Paste {
        at: u64,
        replace: Option<Range>,
        clip: Clipboard,
    },
    /// Attenuate, remove or heal a box in time (`range`) and frequency (`low_hz`..`high_hz`):
    /// the spectral view's repairs (dev plan 10.5).
    Spectral {
        range: Range,
        channels: Option<Vec<usize>>,
        low_hz: f64,
        high_hz: f64,
        op: SpectralOp,
    },
    /// Replace `range` with `channels` (exactly as long, one per channel): an effects rack's
    /// render, named for its history.
    Replace {
        range: Range,
        channels: Vec<Vec<f32>>,
        name: String,
    },
}

impl Edit {
    /// The edit's name in the file's history ("Amplify +6.0 dB").
    pub fn name(&self) -> String {
        match self {
            Edit::Delete { .. } => "Delete".into(),
            Edit::Trim { .. } => "Trim to selection".into(),
            Edit::InsertSilence { .. } => "Insert silence".into(),
            Edit::Silence { .. } => "Silence".into(),
            Edit::Amplify { db, .. } => format!("Amplify {db:+.1} dB"),
            Edit::NormalizePeak { peak_db, .. } => format!("Normalize to {peak_db:.1} dBFS"),
            Edit::NormalizeLoudness { lufs, .. } => format!("Normalize to {lufs:.1} LUFS"),
            Edit::FadeIn { .. } => "Fade in".into(),
            Edit::FadeOut { .. } => "Fade out".into(),
            Edit::Reverse { .. } => "Reverse".into(),
            Edit::Invert { .. } => "Invert".into(),
            Edit::Paste {
                replace: Some(_), ..
            } => "Paste over".into(),
            Edit::Paste { .. } => "Paste".into(),
            Edit::Spectral { op, .. } => match op {
                SpectralOp::Attenuate { db } => format!("Spectral attenuate {db:+.1} dB"),
                SpectralOp::Remove => "Spectral remove".into(),
                SpectralOp::Heal => "Spectral heal".into(),
            },
            Edit::Replace { name, .. } => name.clone(),
        }
    }
}

/// Apply `edit` to `channels` (at `rate`), returning the new channels.
pub fn apply(channels: &[Vec<f32>], rate: u32, edit: &Edit) -> Result<Vec<Vec<f32>>> {
    let len = channels.first().map_or(0, Vec::len) as u64;
    let check = |(start, end): Range| -> Result<(usize, usize)> {
        if end <= start {
            bail!("the range must end after it starts ({start}..{end})");
        }
        if end > len {
            bail!("the range {start}..{end} runs past the file's end ({len} samples)");
        }
        Ok((start as usize, end as usize))
    };
    let whole = |range: &Option<Range>| range.map_or(Ok((0, len as usize)), check);
    let chosen = |list: &Option<Vec<usize>>| -> Result<Vec<usize>> {
        match list {
            None => Ok((0..channels.len()).collect()),
            Some(list) => {
                if let Some(bad) = list.iter().find(|&&c| c >= channels.len()) {
                    bail!(
                        "the file has channels 0 to {}; there is no channel {bad}",
                        channels.len().saturating_sub(1)
                    );
                }
                Ok(list.clone())
            }
        }
    };
    // Apply `f` to each chosen channel's range, in place, on a copy.
    let per_channel = |range: (usize, usize),
                       list: &Option<Vec<usize>>,
                       f: &dyn Fn(&mut [f32])|
     -> Result<Vec<Vec<f32>>> {
        let mut out = channels.to_vec();
        for c in chosen(list)? {
            f(&mut out[c][range.0..range.1]);
        }
        Ok(out)
    };
    let gain_db = |db: f64| 10f64.powf(db / 20.0) as f32;
    match edit {
        Edit::Delete { range } => {
            let (a, b) = check(*range)?;
            Ok(channels
                .iter()
                .map(|c| c[..a].iter().chain(&c[b..]).copied().collect())
                .collect())
        }
        Edit::Trim { range } => {
            let (a, b) = check(*range)?;
            Ok(channels.iter().map(|c| c[a..b].to_vec()).collect())
        }
        Edit::InsertSilence { at, length } => {
            if *at > len {
                bail!("{at} is past the file's end ({len} samples)");
            }
            let at = *at as usize;
            Ok(channels
                .iter()
                .map(|c| {
                    let mut out = Vec::with_capacity(c.len() + *length as usize);
                    out.extend_from_slice(&c[..at]);
                    out.resize(at + *length as usize, 0.0);
                    out.extend_from_slice(&c[at..]);
                    out
                })
                .collect())
        }
        Edit::Silence {
            range,
            channels: list,
        } => per_channel(whole(range)?, list, &|s| s.fill(0.0)),
        Edit::Amplify {
            range,
            channels: list,
            db,
        } => {
            let g = gain_db(*db);
            per_channel(whole(range)?, list, &|s| s.iter_mut().for_each(|v| *v *= g))
        }
        Edit::NormalizePeak {
            range,
            channels: list,
            peak_db,
        } => {
            let range = whole(range)?;
            let peak = chosen(list)?
                .iter()
                .flat_map(|&c| &channels[c][range.0..range.1])
                .fold(0.0f32, |m, v| m.max(v.abs()));
            if peak == 0.0 {
                bail!("the range is silent, so there is nothing to normalize");
            }
            let g = (gain_db(*peak_db) as f64 / f64::from(peak)) as f32;
            per_channel(range, list, &|s| s.iter_mut().for_each(|v| *v *= g))
        }
        Edit::NormalizeLoudness {
            range,
            channels: list,
            lufs,
        } => {
            let range = whole(range)?;
            let picked = chosen(list)?;
            let mut meter = LoudnessMeter::new(rate);
            let left = &channels[picked[0]][range.0..range.1];
            let right = picked.get(1).map(|&c| &channels[c][range.0..range.1]);
            meter.process(left, right);
            let Some(measured) = meter.readings().integrated else {
                bail!(
                    "the range is too short or too quiet to measure: loudness needs at least \
                     400 ms above -70 LUFS"
                );
            };
            let g = gain_db(lufs - measured);
            per_channel(range, list, &|s| s.iter_mut().for_each(|v| *v *= g))
        }
        Edit::FadeIn {
            range,
            channels: list,
            shape,
        } => {
            let range = check(*range)?;
            let n = (range.1 - range.0) as f32;
            per_channel(range, list, &|s| {
                for (i, v) in s.iter_mut().enumerate() {
                    *v *= shape.gain(i as f32 / n);
                }
            })
        }
        Edit::FadeOut {
            range,
            channels: list,
            shape,
        } => {
            let range = check(*range)?;
            let n = (range.1 - range.0) as f32;
            per_channel(range, list, &|s| {
                for (i, v) in s.iter_mut().enumerate() {
                    *v *= shape.gain(1.0 - (i + 1) as f32 / n);
                }
            })
        }
        Edit::Reverse {
            range,
            channels: list,
        } => per_channel(whole(range)?, list, &|s| s.reverse()),
        Edit::Invert {
            range,
            channels: list,
        } => per_channel(whole(range)?, list, &|s| {
            s.iter_mut().for_each(|v| *v = -*v)
        }),
        Edit::Spectral {
            range,
            channels: list,
            low_hz,
            high_hz,
            op,
        } => {
            let b = SpectralBox {
                start: range.0,
                end: range.1,
                low_hz: *low_hz,
                high_hz: *high_hz,
            };
            spectral_edit::check(&b, len, rate)?;
            let mut out = channels.to_vec();
            for c in chosen(list)? {
                out[c] = spectral_edit::process(&channels[c], rate, &b, *op)?;
            }
            Ok(out)
        }
        Edit::Replace {
            range,
            channels: new,
            ..
        } => {
            let (a, b) = check(*range)?;
            if new.len() != channels.len() || new.iter().any(|c| c.len() != b - a) {
                bail!(
                    "the replacement must be {} channels of {} samples",
                    channels.len(),
                    b - a
                );
            }
            Ok(channels
                .iter()
                .zip(new)
                .map(|(c, n)| c[..a].iter().chain(n).chain(&c[b..]).copied().collect())
                .collect())
        }
        Edit::Paste { at, replace, clip } => {
            if clip.channels.is_empty() {
                bail!("nothing has been copied");
            }
            let (a, b) = match replace {
                Some(range) => check(*range)?,
                None if *at > len => bail!("{at} is past the file's end ({len} samples)"),
                None => (*at as usize, *at as usize),
            };
            // At this file's rate; its channels by index, the clip's last one repeating (mono
            // into stereo fills both).
            let source: Vec<Vec<f32>> = clip
                .channels
                .iter()
                .map(|c| {
                    if clip.sample_rate == rate {
                        c.clone()
                    } else {
                        crate::media::resample(c, clip.sample_rate, rate)
                    }
                })
                .collect();
            Ok(channels
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let piece = &source[i.min(source.len() - 1)];
                    c[..a].iter().chain(piece).chain(&c[b..]).copied().collect()
                })
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ten() -> Vec<Vec<f32>> {
        vec![
            (0..10).map(|i| i as f32 / 10.0).collect(),
            (0..10).map(|i| -(i as f32) / 10.0).collect(),
        ]
    }

    fn run(edit: Edit) -> Vec<Vec<f32>> {
        apply(&ten(), 48_000, &edit).unwrap()
    }

    #[test]
    fn length_edits_change_every_channel() {
        let out = run(Edit::Delete { range: (2, 5) });
        assert_eq!(out[0], [0.0, 0.1, 0.5, 0.6, 0.7, 0.8, 0.9]);
        assert_eq!(out[1][2], -0.5);
        let out = run(Edit::Trim { range: (2, 5) });
        assert_eq!(out[0], [0.2, 0.3, 0.4]);
        let out = run(Edit::InsertSilence { at: 1, length: 2 });
        assert_eq!(&out[0][..5], [0.0, 0.0, 0.0, 0.1, 0.2]);
        assert_eq!(out[1].len(), 12);
    }

    #[test]
    fn level_edits_touch_only_their_range_and_channels() {
        let out = run(Edit::Silence {
            range: Some((3, 5)),
            channels: Some(vec![1]),
        });
        assert_eq!(out[0], ten()[0], "channel 0 untouched");
        assert_eq!(&out[1][2..6], [-0.2, 0.0, 0.0, -0.5]);
        let out = run(Edit::Amplify {
            range: None,
            channels: None,
            db: 20.0 * 2f64.log10(),
        });
        for (a, b) in out[0].iter().zip(&ten()[0]) {
            assert!((a - 2.0 * b).abs() < 1e-6, "{a} vs {}", 2.0 * b);
        }
        let out = run(Edit::NormalizePeak {
            range: None,
            channels: None,
            peak_db: 0.0,
        });
        assert!((out[0][9] - 1.0).abs() < 1e-6 && (out[1][9] + 1.0).abs() < 1e-6);
        let out = run(Edit::Reverse {
            range: Some((0, 3)),
            channels: Some(vec![0]),
        });
        assert_eq!(&out[0][..4], [0.2, 0.1, 0.0, 0.3]);
        let out = run(Edit::Invert {
            range: Some((8, 10)),
            channels: None,
        });
        assert_eq!(&out[0][7..], [0.7, -0.8, -0.9]);
        assert_eq!(&out[1][8..], [0.8, 0.9]);
    }

    #[test]
    fn fades_run_from_silence_and_to_silence() {
        let out = run(Edit::FadeIn {
            range: (0, 10),
            channels: None,
            shape: FadeShape::Linear,
        });
        assert_eq!(out[0][0], 0.0);
        assert!((out[0][5] - 0.5 * 0.5).abs() < 1e-6);
        let out = run(Edit::FadeOut {
            range: (5, 10),
            channels: None,
            shape: FadeShape::Linear,
        });
        assert_eq!(out[0][9], 0.0, "the last sample is silent");
        assert_eq!(&out[0][..5], &ten()[0][..5], "before the fade, untouched");
    }

    #[test]
    fn pasting_inserts_or_replaces_and_fills_channels() {
        let clip = Clipboard {
            sample_rate: 48_000,
            channels: vec![vec![9.0, 9.0]],
        };
        let out = run(Edit::Paste {
            at: 2,
            replace: None,
            clip: clip.clone(),
        });
        assert_eq!(&out[0][..5], [0.0, 0.1, 9.0, 9.0, 0.2]);
        assert_eq!(&out[1][..5], [0.0, -0.1, 9.0, 9.0, -0.2], "mono fills both");
        let out = run(Edit::Paste {
            at: 0,
            replace: Some((1, 9)),
            clip,
        });
        assert_eq!(out[0], [0.0, 9.0, 9.0, 0.9]);
    }

    #[test]
    fn loudness_normalizing_reaches_its_target() {
        let rate = 48_000;
        let tone: Vec<f32> = (0..rate * 3)
            .map(|i| 0.1 * (2.0 * std::f32::consts::PI * 997.0 * i as f32 / rate as f32).sin())
            .collect();
        let out = apply(
            &[tone],
            rate as u32,
            &Edit::NormalizeLoudness {
                range: None,
                channels: None,
                lufs: -16.0,
            },
        )
        .unwrap();
        let mut meter = LoudnessMeter::new(rate as u32);
        meter.process(&out[0], None);
        let got = meter.readings().integrated.unwrap();
        assert!((got + 16.0).abs() < 0.1, "{got} LUFS");
    }

    #[test]
    fn bad_ranges_and_channels_are_refused() {
        let refuse = |edit: Edit, says: &str| {
            let err = apply(&ten(), 48_000, &edit).unwrap_err().to_string();
            assert!(err.contains(says), "{err}");
        };
        refuse(Edit::Delete { range: (5, 5) }, "after");
        refuse(Edit::Delete { range: (5, 11) }, "past the file's end");
        refuse(
            Edit::Silence {
                range: None,
                channels: Some(vec![2]),
            },
            "no channel 2",
        );
        refuse(
            Edit::InsertSilence { at: 11, length: 1 },
            "past the file's end",
        );
        refuse(
            Edit::NormalizePeak {
                range: Some((0, 1)),
                channels: None,
                peak_db: 0.0,
            },
            "silent",
        );
        refuse(
            Edit::NormalizeLoudness {
                range: Some((0, 10)),
                channels: None,
                lufs: -16.0,
            },
            "400 ms",
        );
    }
}
