//! What the audio thread publishes for the editor: lock-free atomics,
//! written once per block, read once per frame.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct Shared {
    notes_lo: AtomicU64,
    notes_hi: AtomicU64,
    latency: AtomicU32,
    sample_rate_bits: AtomicU64,
    /// Host tempo, BPM (f32 bits).
    tempo: AtomicU32,
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
    /// Notes the harmony voices sing (Synth mode).
    harmony_lo: AtomicU64,
    harmony_hi: AtomicU64,
    /// Notes the vocoder's carrier plays (Synth mode).
    vocoder_lo: AtomicU64,
    vocoder_hi: AtomicU64,
    /// Gain reduction of the input compressor and de-esser (f32 dB bits).
    comp_db: AtomicU32,
    deess_db: AtomicU32,
    out_comp_db: AtomicU32,
    out_deess_db: AtomicU32,
    /// What each bell of the output EQ cuts dynamically (f32 dB bits).
    eq_dyn_db: [AtomicU32; 3],
    /// Notes the MIDI output plays.
    midi_out_lo: AtomicU64,
    midi_out_hi: AtomicU64,
    /// A Quackapella Link sends the mix, the carrier.
    link_mix: AtomicBool,
    link_carrier: AtomicBool,
    /// Key detection: guess (tonic + 12 if minor, +1; 0 = none),
    /// confidence and progress (f32 bits), a mix is heard.
    key_guess: AtomicU32,
    key_confidence: AtomicU32,
    key_progress: AtomicU32,
    key_mix: AtomicBool,
    /// Analysis taps the engine writes and the editor's spectrum reads.
    pub taps: quack_dsp::tap::Taps,
}

impl Shared {
    pub fn publish_notes(&self, mask: u128) {
        #[allow(clippy::cast_possible_truncation)]
        self.notes_lo.store(mask as u64, Ordering::Relaxed);
        self.notes_hi.store((mask >> 64) as u64, Ordering::Relaxed);
    }

    pub fn publish_tempo(&self, bpm: f64) {
        #[allow(clippy::cast_possible_truncation)]
        self.tempo.store((bpm as f32).to_bits(), Ordering::Relaxed);
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

    pub fn publish_harmony(&self, mask: u128) {
        #[allow(clippy::cast_possible_truncation)]
        self.harmony_lo.store(mask as u64, Ordering::Relaxed);
        self.harmony_hi
            .store((mask >> 64) as u64, Ordering::Relaxed);
    }

    pub fn publish_vocoder(&self, mask: u128) {
        #[allow(clippy::cast_possible_truncation)]
        self.vocoder_lo.store(mask as u64, Ordering::Relaxed);
        self.vocoder_hi
            .store((mask >> 64) as u64, Ordering::Relaxed);
    }

    pub fn publish_reduction(&self, (comp, deess): (f32, f32)) {
        self.comp_db.store(comp.to_bits(), Ordering::Relaxed);
        self.deess_db.store(deess.to_bits(), Ordering::Relaxed);
    }

    pub fn publish_output_reduction(&self, (comp, deess): (f32, f32)) {
        self.out_comp_db.store(comp.to_bits(), Ordering::Relaxed);
        self.out_deess_db.store(deess.to_bits(), Ordering::Relaxed);
    }

    pub fn publish_eq_dynamic(&self, cut: [f32; 3]) {
        for (slot, db) in self.eq_dyn_db.iter().zip(cut) {
            slot.store(db.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn publish_midi_out(&self, mask: u128) {
        #[allow(clippy::cast_possible_truncation)]
        self.midi_out_lo.store(mask as u64, Ordering::Relaxed);
        self.midi_out_hi
            .store((mask >> 64) as u64, Ordering::Relaxed);
    }

    pub fn publish_link(&self, mix: bool, carrier: bool) {
        self.link_mix.store(mix, Ordering::Relaxed);
        self.link_carrier.store(carrier, Ordering::Relaxed);
        self.taps.linked.store(mix, Ordering::Relaxed);
    }

    pub fn publish_key(&self, r: quack_dsp::key::KeyReading) {
        let g = r
            .guess
            .map_or(0, |g| u32::from(g.tonic) + if g.minor { 13 } else { 1 });
        self.key_guess.store(g, Ordering::Relaxed);
        self.key_confidence
            .store(r.confidence.to_bits(), Ordering::Relaxed);
        self.key_progress
            .store(r.progress.to_bits(), Ordering::Relaxed);
        self.key_mix.store(r.mix, Ordering::Relaxed);
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
            tempo: f32::from_bits(self.tempo.load(Ordering::Relaxed)),
            pitch_hz: f32::from_bits(self.pitch_hz.load(Ordering::Relaxed)),
            pitch_confidence: f32::from_bits(self.pitch_confidence.load(Ordering::Relaxed)),
            tune_sung: f32::from_bits(self.tune_sung.load(Ordering::Relaxed)),
            tune_target: f32::from_bits(self.tune_target.load(Ordering::Relaxed)),
            tune_out: f32::from_bits(self.tune_out.load(Ordering::Relaxed)),
            #[allow(clippy::cast_possible_truncation)]
            tune_mask: self.tune_mask.load(Ordering::Relaxed) as u16,
            frozen: self.frozen.load(Ordering::Relaxed),
            harmony_notes: u128::from(self.harmony_lo.load(Ordering::Relaxed))
                | (u128::from(self.harmony_hi.load(Ordering::Relaxed)) << 64),
            vocoder_notes: u128::from(self.vocoder_lo.load(Ordering::Relaxed))
                | (u128::from(self.vocoder_hi.load(Ordering::Relaxed)) << 64),
            comp_db: f32::from_bits(self.comp_db.load(Ordering::Relaxed)),
            deess_db: f32::from_bits(self.deess_db.load(Ordering::Relaxed)),
            out_comp_db: f32::from_bits(self.out_comp_db.load(Ordering::Relaxed)),
            out_deess_db: f32::from_bits(self.out_deess_db.load(Ordering::Relaxed)),
            eq_dyn_db: std::array::from_fn(|i| {
                f32::from_bits(self.eq_dyn_db[i].load(Ordering::Relaxed))
            }),
            midi_out_notes: u128::from(self.midi_out_lo.load(Ordering::Relaxed))
                | (u128::from(self.midi_out_hi.load(Ordering::Relaxed)) << 64),
            link: [
                self.link_mix.load(Ordering::Relaxed),
                self.link_carrier.load(Ordering::Relaxed),
            ],
            key: match self.key_guess.load(Ordering::Relaxed) {
                0 => None,
                #[allow(clippy::cast_possible_truncation)]
                g => Some((((g - 1) % 12) as u8, g > 12)),
            },
            key_confidence: f32::from_bits(self.key_confidence.load(Ordering::Relaxed)),
            key_progress: f32::from_bits(self.key_progress.load(Ordering::Relaxed)),
            key_mix: self.key_mix.load(Ordering::Relaxed),
        }
    }
}
