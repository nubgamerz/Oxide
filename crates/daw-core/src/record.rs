//! Writing recorded input to disk off the audio thread.

use crate::engine::{RecordJob, RecordTake};
use crate::media::PEAK_BLOCK;
use crate::project::TrackId;
use anyhow::{Context, Result};
use hound::{SampleFormat, WavSpec, WavWriter};
use rtrb::{Consumer, Producer, RingBuffer};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// One armed track to record.
pub struct TakeSpec {
    pub track: TrackId,
    pub channel: usize,
    pub stereo: bool,
    pub path: PathBuf,
}

/// A finished recording file.
#[derive(Debug)]
pub struct RecordedFile {
    pub track: TrackId,
    pub path: PathBuf,
    pub frames: u64,
    pub error: Option<String>,
}

/// Where the transport starts for a count-in, and where the record tap engages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountIn {
    /// Locate here...
    pub locate: u64,
    /// ...then play this many samples before it (the part of the count-in before 0).
    pub pre_roll: u64,
    /// Engine position at which the tap engages ([`RecordJob::from`]).
    pub from: u64,
    /// Heard position the click sounds until ([`RecordJob::count_in_to`]).
    pub click_until: i64,
}

/// Plan a count-in `length` samples long (the bars before the record point, from the tempo
/// map) ending at the record point `at`, with `latency` samples of recording shift. Takes are
/// placed `latency` before the engine position they were tapped at (what you heard then), so
/// the tap starts that much after the record point, which puts the take exactly at `at`.
pub fn count_in(at: u64, length: f64, latency: u64) -> CountIn {
    let start = at as i64 - length.round() as i64;
    CountIn {
        locate: start.max(0) as u64,
        pre_roll: (-start).max(0) as u64,
        from: at + latency,
        click_until: at as i64,
    }
}

/// Narrow a job's tap (`from`, engine positions) to a punch range on the timeline: the tap
/// opens at punch-in or `from`, whichever is later, and closes at punch-out. Both ends move by
/// `latency`, as `from` does, so the take sits exactly on the range. `None` when there is
/// nothing left to record.
pub fn punch(from: u64, range: (u64, u64), latency: u64) -> Option<(u64, u64)> {
    let (start, end) = (range.0 + latency, range.1 + latency);
    let from = from.max(start);
    (from < end).then_some((from, end))
}

/// One pass of a loop recording, as a take of the clip that spans the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pass {
    /// Where in the recorded file the pass begins.
    pub offset: u64,
    /// Whether the recording ran to the pass's end (Stop came later).
    pub complete: bool,
}

/// A loop recording cut into passes: the clip they share and one take per pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopTakes {
    pub start: u64,
    pub length: u64,
    pub passes: Vec<Pass>,
    /// The last complete pass, which plays; the last pass if none completed.
    pub active: usize,
}

/// Cut a continuous loop recording of `frames` samples into passes. `heard_start` is the
/// timeline position of the file's first sample (the tap opens at the loop start, so it is
/// `loop_range.0`; anything else is not a loop recording and gets `None`). With a punch
/// range, each pass is trimmed to where it overlaps the loop.
pub fn slice_passes(
    frames: u64,
    heard_start: u64,
    loop_range: (u64, u64),
    punch: Option<(u64, u64)>,
) -> Option<LoopTakes> {
    let (loop_start, loop_end) = loop_range;
    if heard_start != loop_start || loop_end <= loop_start {
        return None;
    }
    let (start, end) = match punch {
        Some((a, b)) => (a.max(loop_start), b.min(loop_end)),
        None => (loop_start, loop_end),
    };
    if end <= start {
        return None;
    }
    let pass_length = loop_end - loop_start;
    let into = start - loop_start;
    let mut passes = Vec::new();
    for k in 0.. {
        let offset = k * pass_length + into;
        if offset >= frames {
            break;
        }
        passes.push(Pass {
            offset,
            complete: offset + (end - start) <= frames,
        });
    }
    let last = passes.len().checked_sub(1)?;
    let active = passes.iter().rposition(|p| p.complete).unwrap_or(last);
    // A single unfinished pass is just a take: end the clip where its audio does.
    let length = match passes.as_slice() {
        [only] if !only.complete => frames - only.offset,
        _ => end - start,
    };
    Some(LoopTakes {
        start,
        length,
        passes,
        active,
    })
}

/// Create the WAV files and the disk-writer thread. The returned job goes to the engine; when
/// the engine drops its producers (recording stopped), the writer finalizes each file and
/// reports it on the receiver. Nothing draws these takes while they record ([`begin`] does).
pub fn start(
    takes: Vec<TakeSpec>,
    sample_rate: u32,
) -> Result<(Box<RecordJob>, Receiver<RecordedFile>)> {
    let started = open(takes, sample_rate, false)?;
    Ok((started.job, started.files))
}

/// A recording set up by [`begin`]: the job for the engine, the finished files, and each take's
/// waveform as it records, in the job's order.
pub struct Started {
    pub job: Box<RecordJob>,
    pub files: Receiver<RecordedFile>,
    pub peaks: Vec<LivePeaks>,
}

/// [`start`], and each take's peaks sent from the audio thread as it records (21.1), for the
/// arrangement to draw the take growing.
pub fn begin(takes: Vec<TakeSpec>, sample_rate: u32) -> Result<Started> {
    open(takes, sample_rate, true)
}

fn open(takes: Vec<TakeSpec>, sample_rate: u32, peaks: bool) -> Result<Started> {
    let mut job_takes = Vec::new();
    let mut live = Vec::new();
    let mut writers = Vec::new();
    for take in takes {
        if let Some(dir) = take.path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let channels = if take.stereo { 2 } else { 1 };
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let writer = WavWriter::create(&take.path, spec)
            .with_context(|| format!("creating {}", take.path.display()))?;
        // Ten seconds of buffer: the writer would have to stall that long to lose audio.
        let (tx, rx) = RingBuffer::new(sample_rate as usize * channels as usize * 10);
        let tap = peaks.then(|| {
            let (tap, rx) = peak_ring(sample_rate);
            live.push(LivePeaks {
                track: take.track,
                name: take
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                channels: usize::from(channels),
                rx,
                peaks: vec![Vec::new(); usize::from(channels)],
                frames: 0,
            });
            tap
        });
        job_takes.push(RecordTake {
            track: take.track,
            channel: take.channel,
            stereo: take.stereo,
            producer: tx,
            peaks: tap,
        });
        writers.push(Writer {
            track: take.track,
            path: take.path,
            channels: channels as u64,
            rx,
            writer: Some(writer),
            samples: 0,
            error: None,
        });
    }
    let (done_tx, done_rx) = channel();
    std::thread::Builder::new()
        .name("disk-writer".into())
        .spawn(move || {
            while !writers.is_empty() {
                std::thread::sleep(Duration::from_millis(15));
                writers.retain_mut(|w| {
                    w.drain();
                    if w.rx.is_abandoned() && w.rx.is_empty() {
                        let _ = done_tx.send(w.finish());
                        false
                    } else {
                        true
                    }
                });
            }
        })
        .context("starting disk writer")?;
    Ok(Started {
        job: Box::new(RecordJob {
            takes: job_takes,
            from: 0,
            to: u64::MAX,
            count_in_to: None,
            open: false,
            exact_from: false,
            start: None,
            dropped: 0,
            midi: None,
            midi_dropped: 0,
            wraps: 0,
        }),
        files: done_rx,
        peaks: live,
    })
}

/// How many peak bins a take's ring holds: a minute's worth at the recording's rate (11,251
/// bins, about 270 KB, at 48 kHz). The main thread drains it every frame (`Session::update`,
/// from the app's `logic`, which runs while minimised too), about three bins a frame, so it
/// fills only if the main thread stalls for a minute; then bins are dropped and counted, never
/// waited for, and the finished file's own peaks replace them all on stop.
pub fn peak_ring_bins(sample_rate: u32) -> usize {
    sample_rate as usize * 60 / PEAK_BLOCK + 1
}

/// A take's peak ring: the audio thread's end and the main thread's.
pub fn peak_ring(sample_rate: u32) -> (PeakTap, Consumer<PeakBin>) {
    let (tx, rx) = RingBuffer::new(peak_ring_bins(sample_rate));
    (PeakTap::new(tx), rx)
}

/// One bin of a take's waveform: the lowest and highest sample of each channel over
/// [`PEAK_BLOCK`] frames of the file (fewer in the last), as [`crate::media::AudioData`]'s
/// peaks have them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeakBin {
    /// Which bin of the file: it covers frames from `index * PEAK_BLOCK`.
    pub index: u64,
    /// Frames in it: `PEAK_BLOCK`, or fewer in the last one.
    pub frames: u32,
    pub lo: [f32; 2],
    pub hi: [f32; 2],
}

/// The audio thread's end of a take's peaks: it folds every frame written to the file into the
/// bin being filled and sends each full bin. Bins are aligned to the file's first frame, not to
/// the engine's blocks, and folded with the same `min`/`max` as `media`'s peaks, so they are
/// bit for bit the finished file's. Allocation-free; a full ring drops the bin and counts it.
pub struct PeakTap {
    producer: Producer<PeakBin>,
    lo: [f32; 2],
    hi: [f32; 2],
    filled: usize,
    index: u64,
    /// Bins that didn't fit in the ring (the main thread stalled).
    pub dropped: u64,
}

impl PeakTap {
    pub fn new(producer: Producer<PeakBin>) -> Self {
        Self {
            producer,
            lo: [f32::MAX; 2],
            hi: [f32::MIN; 2],
            filled: 0,
            index: 0,
            dropped: 0,
        }
    }

    /// One frame as written to the file: `right` is `left` again for a mono take.
    #[inline]
    pub fn add(&mut self, left: f32, right: f32) {
        self.lo = [self.lo[0].min(left), self.lo[1].min(right)];
        self.hi = [self.hi[0].max(left), self.hi[1].max(right)];
        self.filled += 1;
        if self.filled == PEAK_BLOCK {
            self.send();
        }
    }

    /// Send the bin being filled, if it holds anything: the file's last bin, when the engine
    /// lets go of the recording.
    pub fn flush(&mut self) {
        if self.filled > 0 {
            self.send();
        }
    }

    fn send(&mut self) {
        let bin = PeakBin {
            index: self.index,
            frames: self.filled as u32,
            lo: self.lo,
            hi: self.hi,
        };
        if self.producer.push(bin).is_err() {
            self.dropped += 1;
        }
        self.index += 1;
        self.filled = 0;
        self.lo = [f32::MAX; 2];
        self.hi = [f32::MIN; 2];
    }
}

/// The main thread's end of a take's peaks: what has arrived so far, laid out as
/// [`crate::media::AudioData::peaks`] (one `(min, max)` per [`PEAK_BLOCK`] frames per channel).
pub struct LivePeaks {
    pub track: TrackId,
    /// The file's name without its extension, which the finished clip is named.
    pub name: String,
    /// 1 or 2, as the file has.
    pub channels: usize,
    rx: Consumer<PeakBin>,
    pub peaks: Vec<Vec<(f32, f32)>>,
    /// Frames of the file the peaks cover.
    pub frames: u64,
}

impl LivePeaks {
    /// A take's main-thread end from its ring, for tests and checks that build a
    /// [`RecordTake`] themselves.
    pub fn new(track: TrackId, channels: usize, rx: Consumer<PeakBin>) -> Self {
        Self {
            track,
            name: String::new(),
            channels,
            rx,
            peaks: vec![Vec::new(); channels],
            frames: 0,
        }
    }

    /// Take in every bin the audio thread has sent. A bin after dropped ones leaves a flat gap
    /// where they were.
    pub fn drain(&mut self) {
        while let Ok(bin) = self.rx.pop() {
            let at = bin.index as usize;
            for (c, peaks) in self.peaks.iter_mut().enumerate() {
                peaks.resize(at, (0.0, 0.0));
                peaks.push((bin.lo[c], bin.hi[c]));
            }
            self.frames = bin.index * PEAK_BLOCK as u64 + u64::from(bin.frames);
        }
    }
}

/// Where a take still recording shows on the timeline: `(start, offset, length)`, the clip it
/// becomes (its start, where in the file it begins, how long) as far as it has come.
/// `heard_start` is the timeline position of the file's first frame (the engine's tap start
/// less the record shift). A loop recording shows the pass being recorded, on the clip the
/// passes share ([`slice_passes`]), and nothing until the first pass reaches the punch range.
pub fn growing(
    frames: u64,
    heard_start: u64,
    loop_range: Option<(u64, u64)>,
    punch: Option<(u64, u64)>,
) -> (u64, u64, u64) {
    match loop_range.filter(|r| r.0 == heard_start) {
        Some(range) => match slice_passes(frames, heard_start, range, punch) {
            Some(takes) => {
                let pass = takes.passes[takes.passes.len() - 1];
                let length = (frames - pass.offset).min(takes.length);
                (takes.start, pass.offset, length)
            }
            None => (heard_start, 0, 0),
        },
        None => (heard_start, 0, frames),
    }
}

struct Writer {
    track: TrackId,
    path: PathBuf,
    channels: u64,
    rx: Consumer<f32>,
    writer: Option<WavWriter<std::io::BufWriter<std::fs::File>>>,
    samples: u64,
    error: Option<String>,
}

impl Writer {
    fn drain(&mut self) {
        let Ok(chunk) = self.rx.read_chunk(self.rx.slots()) else {
            return;
        };
        if let Some(writer) = &mut self.writer {
            let (a, b) = chunk.as_slices();
            for s in a.iter().chain(b) {
                if let Err(e) = writer.write_sample(*s) {
                    self.error.get_or_insert(e.to_string());
                    break;
                }
            }
            self.samples += (a.len() + b.len()) as u64;
        }
        chunk.commit_all();
    }

    fn finish(&mut self) -> RecordedFile {
        if let Some(writer) = self.writer.take() {
            if let Err(e) = writer.finalize() {
                self.error.get_or_insert(e.to_string());
            }
        }
        RecordedFile {
            track: self.track,
            path: self.path.clone(),
            frames: self.samples / self.channels,
            error: self.error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::AudioData;

    /// Frames folded in blocks that never line up with the bins (as a device's buffers and a
    /// punch-in don't) give the finished file's peaks bit for bit, the last partial bin too.
    #[test]
    fn peak_bins_are_the_files_peaks_whatever_the_blocks() {
        let left: Vec<f32> = (0..1000)
            .map(|i| ((i * 7919) % 1000) as f32 / 500.0 - 1.0)
            .collect();
        let right: Vec<f32> = left.iter().map(|s| -s * 0.5).collect();
        let (mut tap, rx) = peak_ring(48_000);
        let mut live = LivePeaks::new(1, 2, rx);
        let mut at = 0;
        for block in [3, 200, 1, 300, 256, 240].iter().cycle() {
            if at >= left.len() {
                break;
            }
            let end = (at + block).min(left.len());
            for i in at..end {
                tap.add(left[i], right[i]);
            }
            at = end;
            live.drain();
        }
        assert_eq!(live.frames, 768, "three whole bins so far");
        tap.flush();
        live.drain();
        let file = AudioData::new(48_000, vec![left, right]);
        assert_eq!(live.peaks, file.peaks);
        assert_eq!(live.frames, 1000);
        assert_eq!(tap.dropped, 0);
    }

    /// A ring the main thread doesn't drain fills, and then bins are dropped and counted, never
    /// waited for; a later bin still lands at its place, the gap left flat.
    #[test]
    fn a_full_peak_ring_drops_bins_and_counts_them() {
        assert_eq!(peak_ring_bins(48_000), 11_251, "a minute at 48 kHz");
        let (tx, rx) = RingBuffer::new(2);
        let mut tap = PeakTap::new(tx);
        let mut live = LivePeaks::new(1, 1, rx);
        for _ in 0..PEAK_BLOCK * 4 {
            tap.add(1.0, 1.0);
        }
        assert_eq!(tap.dropped, 2);
        live.drain();
        for _ in 0..PEAK_BLOCK {
            tap.add(0.5, 0.5);
        }
        tap.flush();
        live.drain();
        assert_eq!(
            live.peaks[0],
            vec![(1.0, 1.0), (1.0, 1.0), (0.0, 0.0), (0.0, 0.0), (0.5, 0.5)]
        );
        assert_eq!(live.frames, 5 * PEAK_BLOCK as u64);
    }

    #[test]
    fn a_take_recording_shows_where_its_clip_will_be() {
        // From where it starts, as long as it is so far.
        assert_eq!(growing(500, 1000, None, None), (1000, 0, 500));
        // A loop recording: the pass being recorded, on the clip the passes share.
        let looped = Some((1000, 3000));
        assert_eq!(growing(500, 1000, looped, None), (1000, 0, 500));
        assert_eq!(growing(4700, 1000, looped, None), (1000, 4000, 700));
        assert_eq!(growing(4000, 1000, looped, None), (1000, 2000, 2000));
        // A punch range inside the loop: nothing before it, then each pass trimmed to it.
        let punch = Some((1500, 2500));
        assert_eq!(growing(300, 1000, looped, punch), (1000, 0, 0));
        assert_eq!(growing(800, 1000, looped, punch), (1500, 500, 300));
        assert_eq!(growing(2700, 1000, looped, punch), (1500, 2500, 200));
        // Started past the loop's start: not a loop recording, a plain take.
        assert_eq!(growing(500, 1200, looped, None), (1200, 0, 500));
    }

    #[test]
    fn loop_recordings_are_cut_into_passes() {
        let pass = |offset, complete| Pass { offset, complete };
        // 3.5 passes of a 2000-sample loop at 1000.
        let takes = slice_passes(7000, 1000, (1000, 3000), None).unwrap();
        assert_eq!((takes.start, takes.length), (1000, 2000));
        assert_eq!(
            takes.passes,
            vec![
                pass(0, true),
                pass(2000, true),
                pass(4000, true),
                pass(6000, false)
            ]
        );
        assert_eq!(takes.active, 2, "the last complete pass plays");
        // Exactly three passes: all complete, the third plays.
        let takes = slice_passes(6000, 1000, (1000, 3000), None).unwrap();
        assert_eq!((takes.passes.len(), takes.active), (3, 2));
        // Stopped inside the first pass: one take, as long as the audio.
        let takes = slice_passes(500, 1000, (1000, 3000), None).unwrap();
        assert_eq!((takes.length, takes.passes), (500, vec![pass(0, false)]));
        // A punch range inside the loop trims every pass to it.
        let takes = slice_passes(7000, 1000, (1000, 3000), Some((1500, 2500))).unwrap();
        assert_eq!((takes.start, takes.length), (1500, 1000));
        let offsets: Vec<u64> = takes.passes.iter().map(|p| p.offset).collect();
        assert_eq!(offsets, vec![500, 2500, 4500, 6500]);
        assert!(!takes.passes[3].complete, "6500 + 1000 runs past 7000");
        assert_eq!(takes.active, 2);
        // Not a loop recording, or a punch range outside the loop: nothing to cut.
        assert_eq!(slice_passes(7000, 1200, (1000, 3000), None), None);
        assert_eq!(
            slice_passes(7000, 1000, (1000, 3000), Some((4000, 5000))),
            None
        );
    }

    #[test]
    fn punch_narrows_the_tap_to_the_range() {
        // From before punch-in: open at punch-in.
        assert_eq!(punch(0, (1000, 2000), 0), Some((1000, 2000)));
        // Already inside (recording started late): open at once, close at punch-out.
        assert_eq!(punch(1500, (1000, 2000), 0), Some((1500, 2000)));
        // Past punch-out: nothing to record.
        assert_eq!(punch(2000, (1000, 2000), 0), None);
        // Latency moves both ends with the tap, so the take lands on the range.
        assert_eq!(punch(810, (1000, 2000), 810), Some((1810, 2810)));
    }

    #[test]
    fn count_in_starts_bars_early_and_before_the_timeline_if_needed() {
        let bar = 96_000.0; // 120 BPM, 4/4, 48 kHz
                            // Well into the song: two bars early, no pre-roll.
        let plan = count_in(500_000, bar * 2.0, 0);
        assert_eq!((plan.locate, plan.pre_roll), (500_000 - 192_000, 0));
        assert_eq!((plan.from, plan.click_until), (500_000, 500_000));
        // At the start: all of it before the timeline.
        let plan = count_in(0, bar * 1.0, 0);
        assert_eq!((plan.locate, plan.pre_roll), (0, 96_000));
        // Half a bar in: part before, part on the timeline.
        let plan = count_in(48_000, bar * 1.0, 0);
        assert_eq!((plan.locate, plan.pre_roll), (0, 48_000));
        // Latency moves the tap, not the click or the start.
        let plan = count_in(500_000, bar * 1.0, 810);
        assert_eq!(plan.locate, 404_000);
        assert_eq!((plan.from, plan.click_until), (500_810, 500_000));
    }
}
