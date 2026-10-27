//! Small lock-free helpers shared between the UI and the audio thread.

use std::sync::atomic::{AtomicU32, Ordering};

/// An `f32` stored in an `AtomicU32`.
#[derive(Debug, Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(value: f32) -> Self {
        Self(AtomicU32::new(value.to_bits()))
    }

    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Raise to `value` if larger. Only valid for non-negative values, whose bit patterns
    /// order the same way as the floats.
    pub fn fetch_max_positive(&self, value: f32) {
        self.0
            .fetch_max(value.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// Read and reset to zero (for peak meters).
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Ordering::Relaxed))
    }
}

pub fn db_to_gain(db: f32) -> f32 {
    if db <= -96.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 1e-5 {
        -100.0
    } else {
        20.0 * gain.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_max_and_take() {
        let a = AtomicF32::new(0.0);
        a.fetch_max_positive(0.5);
        a.fetch_max_positive(0.25);
        assert_eq!(a.take(), 0.5);
        assert_eq!(a.load(), 0.0);
    }

    #[test]
    fn db_round_trip() {
        assert!((gain_to_db(db_to_gain(-6.0)) + 6.0).abs() < 1e-4);
        assert_eq!(db_to_gain(-120.0), 0.0);
    }
}
