//! What the audio thread publishes for the editor: lock-free atomics,
//! written once per block, read once per frame.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct Shared {
    notes_lo: AtomicU64,
    notes_hi: AtomicU64,
    latency: AtomicU32,
    sample_rate_bits: AtomicU64,
}

impl Shared {
    pub fn publish_notes(&self, mask: u128) {
        #[allow(clippy::cast_possible_truncation)]
        self.notes_lo.store(mask as u64, Ordering::Relaxed);
        self.notes_hi.store((mask >> 64) as u64, Ordering::Relaxed);
    }

    pub fn publish_config(&self, latency: u32, sample_rate: f64) {
        self.latency.store(latency, Ordering::Relaxed);
        self.sample_rate_bits
            .store(sample_rate.to_bits(), Ordering::Relaxed);
    }

    #[must_use]
    pub fn view(&self) -> quack_ui::View {
        quack_ui::View {
            held_notes: u128::from(self.notes_lo.load(Ordering::Relaxed))
                | (u128::from(self.notes_hi.load(Ordering::Relaxed)) << 64),
            latency_samples: self.latency.load(Ordering::Relaxed),
            sample_rate: f64::from_bits(self.sample_rate_bits.load(Ordering::Relaxed)),
        }
    }
}
