//! Note targeting for the autotune (roadmap step 4).
//!
//! Pure functions: given a detected pitch, find the note to correct
//! to, from a key and scale or from the notes held on the MIDI input.
//! Retune speed, humanize and the hard-tune effect come with step 4.

use crate::midi::NoteStack;

/// Scales as 12-bit masks over pitch classes, bit 0 = the key's root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Chromatic,
    Major,
    NaturalMinor,
    HarmonicMinor,
    MajorPentatonic,
    MinorPentatonic,
}

impl Scale {
    #[must_use]
    pub const fn mask(self) -> u16 {
        match self {
            Self::Chromatic => 0b1111_1111_1111,
            Self::Major => 0b1010_1011_0101,
            Self::NaturalMinor => 0b0101_1010_1101,
            Self::HarmonicMinor => 0b1001_1010_1101,
            Self::MajorPentatonic => 0b0010_1001_0101,
            Self::MinorPentatonic => 0b0100_1010_1001,
        }
    }
}

/// Nearest note of `scale` in `key` (0 = C .. 11 = B) to the fractional
/// MIDI note `note`. Ties go down.
#[must_use]
pub fn nearest_in_scale(note: f32, key: u8, scale: Scale) -> f32 {
    let mask = scale.mask();
    let centre = note.round();
    let mut best = centre;
    let mut best_d = f32::MAX;
    for off in -6..=6 {
        #[allow(clippy::cast_precision_loss)]
        let cand = centre + off as f32;
        #[allow(clippy::cast_possible_truncation)]
        let pc = (cand as i32 - i32::from(key % 12)).rem_euclid(12);
        if mask & (1 << pc) == 0 {
            continue;
        }
        let d = (cand - note).abs();
        if d < best_d - 1e-6 {
            best = cand;
            best_d = d;
        }
    }
    best
}

/// Nearest held MIDI note to `note`, octave-free: the singer can sing
/// in any octave, the held note gives the pitch class. `None` if no
/// note is held.
#[must_use]
pub fn nearest_held(note: f32, held: &NoteStack) -> Option<f32> {
    let mut mask = 0u16;
    for n in held.held() {
        mask |= 1 << (n.note % 12);
    }
    if mask == 0 {
        return None;
    }
    let centre = note.round();
    let mut best: Option<(f32, f32)> = None;
    for off in -6..=6 {
        #[allow(clippy::cast_precision_loss)]
        let cand = centre + off as f32;
        #[allow(clippy::cast_possible_truncation)]
        let pc = (cand as i32).rem_euclid(12);
        if mask & (1 << pc) != 0 {
            let d = (cand - note).abs();
            if best.is_none_or(|(_, bd)| d < bd - 1e-6) {
                best = Some((cand, d));
            }
        }
    }
    best.map(|(n, _)| n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_the_scale() {
        // C major: C# 61.4 -> D 62 is nearer than C 60? 61.4 is 1.4 from
        // 60 and 0.6 from 62.
        assert_eq!(nearest_in_scale(61.4, 0, Scale::Major), 62.0);
        assert_eq!(nearest_in_scale(60.2, 0, Scale::Major), 60.0);
        // A minor (key 9): G# is out, G is in.
        assert_eq!(nearest_in_scale(67.9, 9, Scale::NaturalMinor), 67.0);
        assert_eq!(nearest_in_scale(67.9, 9, Scale::HarmonicMinor), 68.0);
        assert_eq!(nearest_in_scale(64.4, 0, Scale::Chromatic), 64.0);
    }

    #[test]
    fn follows_held_notes_in_any_octave() {
        let mut s = NoteStack::default();
        assert_eq!(nearest_held(60.0, &s), None);
        s.note_on(0, 43, 100); // G2
        assert_eq!(nearest_held(66.4, &s), Some(67.0)); // sung near G4
        s.note_on(0, 64, 100); // E4
        assert_eq!(nearest_held(65.0, &s), Some(64.0));
    }
}
