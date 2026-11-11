//! Pitch detection (roadmap step 2).
//!
//! Planned: a YIN / pYIN style detector tuned for the voice (about
//! 60 Hz to 1.5 kHz), with a voiced/unvoiced decision so consonants
//! and breath are never "corrected", and a confidence the shifter uses
//! to fade its correction in and out (key to the Natural mode).

/// One pitch reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitchEstimate {
    /// Fundamental frequency in Hz.
    pub hz: f32,
    /// 0 (noise, unvoiced) to 1 (clearly periodic).
    pub confidence: f32,
}

/// Interface of the detectors to come. Implementations allocate in
/// `prepare` only.
pub trait PitchDetector {
    fn prepare(&mut self, sample_rate: f64, max_block: usize);
    fn reset(&mut self);
    /// Feed input samples.
    fn push(&mut self, input: &[f32]);
    /// Latest reading, `None` while unvoiced.
    fn estimate(&self) -> Option<PitchEstimate>;
    /// Samples between an input sample and the reading that includes it.
    fn latency_samples(&self) -> usize;
}

/// Frequency to MIDI note number (69 = A4 = 440 Hz), fractional.
#[must_use]
pub fn hz_to_midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// MIDI note number (fractional) to frequency.
#[must_use]
pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * ((note - 69.0) / 12.0).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_and_hz_round_trip() {
        assert!((midi_to_hz(69.0) - 440.0).abs() < 1e-3);
        assert!((midi_to_hz(60.0) - 261.6256).abs() < 1e-2);
        for n in [40.0, 57.5, 81.25] {
            assert!((hz_to_midi(midi_to_hz(n)) - n).abs() < 1e-4);
        }
    }
}
