//! MIDI-driven harmony voices (roadmap step 6).
//!
//! Planned: one shifted copy of the (corrected) lead per held note, up
//! to [`MAX_VOICES`], each with its own formant handling, pan and
//! humanization (slight timing and pitch drift) so the stack sounds
//! like several singers, not a chord of clones. Voices reuse the step 3
//! shifter, so they share the Natural mode's quality.

/// Most harmony voices at once (fixed, allocated in `prepare`).
pub const MAX_VOICES: usize = 8;
