//! Deterministic stereo test signal.
//!
//! Layout (times scale with the requested duration, given here for 10 s):
//! - 0.0-0.1 s  silence (makes latency visible)
//! - 0.1-3.0 s  pink-noise bursts, independent per channel, rising from about -30 to -4 dB
//! - 3.0-6.0 s  four decaying chord hits, the last peaking near -1 dBFS so limiters engage
//! - 6.0-9.5 s  logarithmic sine sweep 20 Hz-20 kHz, L at -6 dBFS and R at -8 dBFS
//! - 9.5-10 s   silence (room for tails)
//!
//! [`guitar`] is the other signal: a plucked guitar riff, for where the demo needs music.

use std::f64::consts::TAU;

/// xorshift64*: tiny, fast and fully deterministic for a given seed.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // A zero state would stay zero forever.
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [-1, 1).
    pub fn bipolar(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

/// Paul Kellet's pink-noise filter applied to white noise.
struct Pink {
    rng: Rng,
    b: [f64; 7],
}

impl Pink {
    fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
            b: [0.0; 7],
        }
    }

    fn next(&mut self) -> f64 {
        let w = self.rng.bipolar();
        let b = &mut self.b;
        b[0] = 0.99886 * b[0] + w * 0.0555179;
        b[1] = 0.99332 * b[1] + w * 0.0750759;
        b[2] = 0.96900 * b[2] + w * 0.1538520;
        b[3] = 0.86650 * b[3] + w * 0.3104856;
        b[4] = 0.55000 * b[4] + w * 0.5329522;
        b[5] = -0.7616 * b[5] - w * 0.0168980;
        let out = b.iter().sum::<f64>() + w * 0.5362;
        b[6] = w * 0.115926;
        out * 0.11
    }
}

fn db_to_gain(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Linear fade-in/out envelope over `fade` seconds at both ends of a segment of length `len`.
fn edge_fade(t: f64, len: f64, fade: f64) -> f64 {
    (t / fade).min((len - t) / fade).clamp(0.0, 1.0)
}

/// One chord hit at time `th` into the hit: a decaying A-minor-ish stack with 2nd
/// harmonics, low notes leaning left and high notes leaning right. Unnormalized.
fn chord_hit(th: f64, hit_period: f64) -> (f64, f64) {
    // (frequency, left gain, right gain)
    const NOTES: [(f64, f64, f64); 5] = [
        (110.0, 1.0, 0.6),
        (220.0, 0.9, 0.7),
        (261.63, 0.7, 0.9),
        (329.63, 0.6, 1.0),
        (440.0, 0.5, 1.0),
    ];
    let env = (th / 0.005).min(1.0) * (-th / 0.4).exp() * edge_fade(th, hit_period, 0.005);
    let (mut l, mut r) = (0.0, 0.0);
    for &(f, gl, gr) in &NOTES {
        let s = (TAU * f * th).sin() + 0.3 * (TAU * 2.0 * f * th).sin();
        l += s * gl;
        r += s * gr;
    }
    (l * env, r * env)
}

/// Generate the stereo test signal, `[channel][sample]`.
pub fn generate(sample_rate: u32, seconds: f64, seed: u64) -> Vec<Vec<f32>> {
    let sr = f64::from(sample_rate);
    let n = (seconds * sr).round() as usize;
    let k = seconds / 10.0; // scale factor for section times
    let mut out = vec![vec![0.0f32; n]; 2];
    let mut pink = [Pink::new(seed), Pink::new(seed.wrapping_add(1))];

    let noise = (0.1 * k, 3.0 * k);
    let chord = (3.0 * k, 6.0 * k);
    let sweep = (6.0 * k, 9.5 * k);

    let burst_period = 0.3 * k;
    let burst_on = 0.2 * k;
    let bursts = ((noise.1 - noise.0) / burst_period).floor().max(1.0);

    let hit_period = (chord.1 - chord.0) / 4.0;
    let hit_peaks_db = [-12.0, -6.0, -3.0, -1.0];
    // Every hit has the same shape, so measure one and scale each to its exact target peak.
    let hit_len = (hit_period * sr) as usize;
    let unit_peak = (0..hit_len)
        .map(|i| {
            let (l, r) = chord_hit(i as f64 / sr, hit_period);
            l.abs().max(r.abs())
        })
        .fold(0.0, f64::max)
        .max(1e-9);

    let (f0, f1) = (20.0f64, 20_000.0f64);
    let sweep_len = sweep.1 - sweep.0;
    let sweep_rate = (f1 / f0).ln() / sweep_len;

    let (left, right) = out.split_at_mut(1);
    for (i, (ol, or)) in left[0].iter_mut().zip(right[0].iter_mut()).enumerate() {
        let t = i as f64 / sr;
        let (l, r) = if t >= noise.0 && t < noise.1 {
            // Always advance both generators so the noise never depends on the gating.
            let (pl, pr) = (pink[0].next(), pink[1].next());
            let local = t - noise.0;
            let idx = (local / burst_period).floor();
            let in_burst = local - idx * burst_period;
            if in_burst < burst_on {
                let level = -30.0 + 26.0 * idx / (bursts - 1.0).max(1.0);
                let g = db_to_gain(level) * edge_fade(in_burst, burst_on, 0.005);
                (pl * g, pr * g)
            } else {
                (0.0, 0.0)
            }
        } else if t >= chord.0 && t < chord.1 {
            let local = t - chord.0;
            let hit = ((local / hit_period).floor() as usize).min(3);
            let th = local - hit as f64 * hit_period;
            let g = db_to_gain(hit_peaks_db[hit]) / unit_peak;
            let (l, r) = chord_hit(th, hit_period);
            (l * g, r * g)
        } else if t >= sweep.0 && t < sweep.1 {
            let local = t - sweep.0;
            // Exponential sweep phase: integral of f0 * e^(rate * t).
            let phase = TAU * f0 * ((sweep_rate * local).exp() - 1.0) / sweep_rate;
            let s = phase.sin() * edge_fade(local, sweep_len, 0.01);
            (s * db_to_gain(-6.0), s * db_to_gain(-8.0))
        } else {
            (0.0, 0.0)
        };
        *ol = l.clamp(-0.999, 0.999) as f32;
        *or = r.clamp(-0.999, 0.999) as f32;
    }
    out
}

/// One plucked string: a Karplus-Strong loop excited by a filtered noise burst.
struct Pluck {
    /// Where the pluck starts and where the string is damped (samples).
    start: usize,
    end: usize,
    freq: f64,
    amp: f64,
}

/// A chord's strings low to high (MIDI note numbers), as a guitarist would voice it.
const CHORDS: [&[u8]; 4] = [
    &[40, 47, 52, 55, 59, 64], // E minor
    &[48, 52, 55, 60, 64],     // C
    &[43, 47, 50, 55, 59, 67], // G
    &[50, 57, 62, 66],         // D
];

fn midi_hz(note: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(note) - 69.0) / 12.0)
}

/// The riff `guitar` plays: a bar of 2 s per chord (120 bpm in 4/4), from 0.1 s: a down strum
/// on beat 1, four eighth-note picks from the chord's upper strings, an up strum on beat 4.
fn riff(sample_rate: f64, seconds: f64, rng: &mut Rng) -> Vec<Pluck> {
    let bar = 2.0;
    let lead_in = 0.1;
    let mut plucks = Vec::new();
    let mut at = |t: f64, until: f64, note: u8, amp: f64, rng: &mut Rng| {
        // A human hand: a few milliseconds either way, a little louder or softer.
        let t = t + 0.004 * rng.bipolar();
        let amp = amp * (1.0 + 0.15 * rng.bipolar());
        if t < seconds {
            plucks.push(Pluck {
                start: (t * sample_rate) as usize,
                end: ((until.min(seconds)) * sample_rate) as usize,
                freq: midi_hz(note),
                amp,
            });
        }
    };
    let mut k = 0;
    loop {
        let t0 = lead_in + k as f64 * bar;
        if t0 >= seconds {
            break;
        }
        let chord = CHORDS[k % CHORDS.len()];
        let next = t0 + bar;
        // Down strum: low to high, 12 ms apart, ringing to the up strum.
        for (s, &note) in chord.iter().enumerate() {
            at(t0 + 0.012 * s as f64, t0 + 1.5, note, 0.9, rng);
        }
        // Picked eighths on the top three strings, each ringing a beat.
        let top = &chord[chord.len() - 3..];
        for (j, &note) in [top[0], top[1], top[2], top[1]].iter().enumerate() {
            let t = t0 + 0.5 + 0.25 * j as f64;
            at(t, t + 0.5, note, 0.6, rng);
        }
        // Up strum: high to low, quicker and lighter, ringing into the next bar.
        for (s, &note) in chord.iter().rev().enumerate() {
            at(t0 + 1.5 + 0.008 * s as f64, next + 0.05, note, 0.5, rng);
        }
        k += 1;
    }
    plucks
}

/// A deterministic plucked-guitar recording, mono: the demo's audio where a real recording
/// can't be used (the manual's shots without the DI, dev plan 28.1). Chords strummed and
/// picked (`riff`) on Karplus-Strong strings, a faint noise floor under them (so a noise
/// print or a click scan has something to find, as on a real take), saturated a little and
/// normalised to -3 dBFS.
pub fn guitar(sample_rate: u32, seconds: f64, seed: u64) -> Vec<f32> {
    let sr = f64::from(sample_rate);
    let n = (seconds * sr).round() as usize;
    let mut rng = Rng::new(seed);
    let mut out = vec![0.0f64; n];
    for pluck in riff(sr, seconds, &mut rng) {
        // The loop's length in samples (the two-point average adds half a sample); whole
        // samples, a few cents out at most, which a picture of a waveform never shows.
        let len = (sr / pluck.freq - 0.5).round().max(2.0) as usize;
        // Each pass round the loop loses enough to fall 60 dB in about 3 s (less up high).
        let t60 = 3.0 * (110.0 / pluck.freq).powf(0.3);
        let loss = 0.001f64.powf(1.0 / (pluck.freq * t60));
        let mut line = vec![0.0f64; len];
        // The pick: a burst of noise, softened (a pick near the bridge would be brighter).
        let mut lp = 0.0;
        for x in line.iter_mut() {
            lp += 0.5 * (rng.bipolar() - lp);
            *x = lp * pluck.amp;
        }
        let release = (0.03 * sr) as usize;
        let stop = (pluck.end + release).min(n);
        let mut pos = 0usize;
        let mut last = 0.0;
        let mut dc = (0.0, 0.0);
        for (i, o) in out[pluck.start.min(stop)..stop].iter_mut().enumerate() {
            // The string's loop: what comes round is heard, and goes back averaged with the
            // sample before it (the low-pass that makes high harmonics die first).
            let y = line[pos];
            line[pos] = loss * 0.5 * (y + last);
            last = y;
            pos = (pos + 1) % len;
            // The hand damping the string at its end.
            let held = pluck.start + i;
            let env = if held >= pluck.end {
                1.0 - (held - pluck.end) as f64 / release as f64
            } else {
                1.0
            };
            // A DC blocker: the noise burst has some.
            let hp = y - dc.0 + 0.995 * dc.1;
            dc = (y, hp);
            *o += hp * env;
        }
    }
    // A touch of saturation evens the strums and the picks out, as a player's dynamics and a
    // preamp would; then the peak to -3 dBFS.
    let peak = out.iter().fold(0.0f64, |m, x| m.max(x.abs())).max(1e-9);
    for x in out.iter_mut() {
        *x = (2.5 * *x / peak).tanh();
    }
    let peak = out.iter().fold(0.0f64, |m, x| m.max(x.abs())).max(1e-9);
    let gain = db_to_gain(-3.0) / peak;
    let floor = db_to_gain(-66.0);
    out.iter()
        .map(|&x| (x * gain + floor * rng.bipolar()).clamp(-0.999, 0.999) as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guitar_is_deterministic_and_sounds_like_plucks() {
        let a = guitar(44_100, 6.0, 1);
        assert_eq!(a, guitar(44_100, 6.0, 1));
        assert_ne!(a, guitar(44_100, 6.0, 2));
        assert_eq!(a.len(), 6 * 44_100);
        let peak = a.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!((peak - db_to_gain(-3.0) as f32).abs() < 0.01, "peak {peak}");
        // Quiet before the first strum (only the noise floor), loud just after it.
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        let before = rms(&a[..4000]);
        let after = rms(&a[4500..9000]);
        assert!(before < 0.002, "before {before}");
        assert!(after > 0.05, "after {after}");
        // The strings ring and decay: each bar's last quarter is quieter than its first.
        let bar = 2 * 44_100;
        let start = 4410;
        assert!(rms(&a[start + bar * 3 / 4 - 22_050..start + bar * 3 / 4]) < after);
    }

    #[test]
    fn a_short_guitar_still_has_its_first_strum() {
        let a = guitar(48_000, 0.5, 3);
        assert_eq!(a.len(), 24_000);
        assert!(a.iter().any(|x| x.abs() > 0.3));
    }

    #[test]
    fn deterministic_for_a_seed() {
        assert_eq!(generate(8000, 1.0, 7), generate(8000, 1.0, 7));
        assert_ne!(generate(8000, 1.0, 7), generate(8000, 1.0, 8));
    }

    #[test]
    fn loud_but_not_clipping() {
        let s = generate(48_000, 10.0, 1);
        let peak = s.iter().flatten().fold(0.0f32, |m, &x| m.max(x.abs()));
        // The last chord hit is normalized to -1 dBFS and nothing else is louder.
        assert!((peak - db_to_gain(-1.0) as f32).abs() < 1e-4, "peak {peak}");
        assert_eq!(s[0][0], 0.0, "starts with silence");
    }
}
