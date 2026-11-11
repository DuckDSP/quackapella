//! What the audio thread publishes for the editor: lock-free atomics,
//! written once per block, read once per frame.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct Shared {
    notes_lo: AtomicU64,
    notes_hi: AtomicU64,
    latency: AtomicU32,
    sample_rate_bits: AtomicU64,
    /// Detected pitch (f32 bits, 0 while unvoiced) and its confidence.
    pitch_hz: AtomicU32,
    pitch_confidence: AtomicU32,
    /// Autotune: sung, target and output notes (f32 bits, 0 while off
    /// or unvoiced) and the allowed pitch classes.
    tune_sung: AtomicU32,
    tune_target: AtomicU32,
    tune_out: AtomicU32,
    tune_mask: AtomicU32,
    /// A frozen grain is sounding (Creative mode).
    frozen: AtomicBool,
}

impl Shared {
    pub fn publish_notes(&self, mask: u128) {
        #[allow(clippy::cast_possible_truncation)]
        self.notes_lo.store(mask as u64, Ordering::Relaxed);
        self.notes_hi.store((mask >> 64) as u64, Ordering::Relaxed);
    }

    pub fn publish_pitch(&self, pitch: Option<quack_dsp::pitch::PitchEstimate>) {
        let (hz, conf) = pitch.map_or((0.0, 0.0), |p| (p.hz, p.confidence));
        self.pitch_hz.store(hz.to_bits(), Ordering::Relaxed);
        self.pitch_confidence
            .store(conf.to_bits(), Ordering::Relaxed);
    }

    pub fn publish_tune(&self, tune: Option<quack_dsp::autotune::TuneReading>, mask: u16) {
        let t = tune.unwrap_or_default();
        self.tune_sung.store(t.sung.to_bits(), Ordering::Relaxed);
        self.tune_target
            .store(t.target.to_bits(), Ordering::Relaxed);
        self.tune_out.store(t.out.to_bits(), Ordering::Relaxed);
        self.tune_mask.store(u32::from(mask), Ordering::Relaxed);
    }

    pub fn publish_frozen(&self, frozen: bool) {
        self.frozen.store(frozen, Ordering::Relaxed);
    }

    /// The latency moves with the voice range.
    pub fn publish_latency(&self, latency: u32) {
        self.latency.store(latency, Ordering::Relaxed);
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
            pitch_hz: f32::from_bits(self.pitch_hz.load(Ordering::Relaxed)),
            pitch_confidence: f32::from_bits(self.pitch_confidence.load(Ordering::Relaxed)),
            tune_sung: f32::from_bits(self.tune_sung.load(Ordering::Relaxed)),
            tune_target: f32::from_bits(self.tune_target.load(Ordering::Relaxed)),
            tune_out: f32::from_bits(self.tune_out.load(Ordering::Relaxed)),
            #[allow(clippy::cast_possible_truncation)]
            tune_mask: self.tune_mask.load(Ordering::Relaxed) as u16,
            frozen: self.frozen.load(Ordering::Relaxed),
        }
    }
}
