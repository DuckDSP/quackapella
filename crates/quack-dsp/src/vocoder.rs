//! Vocoder and talkbox (roadmap step 7).
//!
//! Planned: a filter-bank vocoder (up to [`MAX_BANDS`] bands) and an
//! LPC talkbox, with an internal carrier played by the MIDI input
//! (saw / pulse / noise), so no external synth is needed live.

/// Most vocoder bands (fixed, allocated in `prepare`).
pub const MAX_BANDS: usize = 32;
