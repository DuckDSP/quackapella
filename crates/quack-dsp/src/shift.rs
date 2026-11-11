//! Pitch shifting with formant preservation (roadmap step 3).
//!
//! The heart of the *Natural* mode. The plan, to be measured against
//! Manipulator on Vincent's material:
//!
//! - pitch-synchronous resynthesis (PSOLA family) driven by the
//!   detector's period marks, for a clean voice with no phasing at
//!   small and large intervals alike;
//! - formants handled separately from pitch: the spectral envelope is
//!   kept in place (Natural, Tune) or moved on its own (Creative),
//!   which avoids the chipmunk / monster colour of plain resampling;
//! - unvoiced segments (s, f, ch, breath) passed through, not shifted;
//! - correction faded by the detector's confidence, so transitions and
//!   vibrato stay human.
//!
//! The shifter's own latency must fit [`crate::latency::LATENCY_MS`];
//! the engine pads the rest.

/// What the shifter is asked to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShiftSettings {
    /// Pitch ratio (2.0 = one octave up).
    pub pitch_ratio: f32,
    /// Formant ratio applied after preservation (1.0 = unchanged).
    pub formant_ratio: f32,
    /// Keep the spectral envelope in place when the pitch moves.
    pub preserve_formants: bool,
}

impl Default for ShiftSettings {
    fn default() -> Self {
        Self {
            pitch_ratio: 1.0,
            formant_ratio: 1.0,
            preserve_formants: true,
        }
    }
}

/// Interface of the shifters to come. Allocation in `prepare` only.
pub trait PitchShifter {
    fn prepare(&mut self, sample_rate: f64, max_block: usize);
    fn reset(&mut self);
    fn set(&mut self, settings: ShiftSettings);
    /// Process one mono block (`input.len() == output.len()`).
    fn process(&mut self, input: &[f32], output: &mut [f32]);
    fn latency_samples(&self) -> usize;
}

/// Identity shifter: the placeholder until step 3.
#[derive(Debug, Default, Clone)]
pub struct Identity;

impl PitchShifter for Identity {
    fn prepare(&mut self, _: f64, _: usize) {}
    fn reset(&mut self) {}
    fn set(&mut self, _: ShiftSettings) {}
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        output.copy_from_slice(input);
    }
    fn latency_samples(&self) -> usize {
        0
    }
}

/// Semitones to pitch ratio.
#[must_use]
pub fn semitones_to_ratio(st: f32) -> f32 {
    (st / 12.0).exp2()
}
