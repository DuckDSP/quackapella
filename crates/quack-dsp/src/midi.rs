//! Notes held on the MIDI input.
//!
//! The autotune (step 4) uses them as target notes and the harmonizer
//! (step 6) as voices. Fixed capacity, no allocation: safe to update
//! from the audio thread at the event's sample offset.

/// Most notes tracked at once; further presses are ignored until one is
/// released.
pub const MAX_HELD: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldNote {
    pub note: u8,
    pub velocity: u8,
    pub channel: u8,
}

/// Held notes in press order (oldest first).
#[derive(Debug, Clone, Copy)]
pub struct NoteStack {
    notes: [HeldNote; MAX_HELD],
    len: usize,
}

impl Default for NoteStack {
    fn default() -> Self {
        Self {
            notes: [HeldNote {
                note: 0,
                velocity: 0,
                channel: 0,
            }; MAX_HELD],
            len: 0,
        }
    }
}

impl NoteStack {
    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8) {
        let note = note.min(127);
        // A repeated press moves the note to the top.
        self.note_off(channel, note);
        if self.len < MAX_HELD {
            self.notes[self.len] = HeldNote {
                note,
                velocity,
                channel,
            };
            self.len += 1;
        }
    }

    pub fn note_off(&mut self, channel: u8, note: u8) {
        if let Some(i) = self
            .held()
            .iter()
            .position(|n| n.note == note && n.channel == channel)
        {
            self.notes.copy_within(i + 1..self.len, i);
            self.len -= 1;
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    #[must_use]
    pub fn held(&self) -> &[HeldNote] {
        &self.notes[..self.len]
    }

    /// Last pressed note still held (mono priority).
    #[must_use]
    pub fn latest(&self) -> Option<HeldNote> {
        self.held().last().copied()
    }

    /// The held notes as a 128-bit mask (bit n = note n), for the editor.
    #[must_use]
    pub fn mask(&self) -> u128 {
        self.held().iter().fold(0, |m, n| m | (1u128 << n.note))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_press_order_and_releases() {
        let mut s = NoteStack::default();
        s.note_on(0, 60, 100);
        s.note_on(0, 64, 90);
        s.note_on(0, 67, 80);
        assert_eq!(s.latest().map(|n| n.note), Some(67));
        s.note_off(0, 67);
        assert_eq!(s.latest().map(|n| n.note), Some(64));
        s.note_on(0, 60, 110);
        assert_eq!(
            s.held().iter().map(|n| n.note).collect::<Vec<_>>(),
            [64, 60]
        );
        assert_eq!(s.mask(), (1 << 60) | (1 << 64));
        s.note_off(1, 60); // other channel: no effect
        assert_eq!(s.held().len(), 2);
    }

    #[test]
    fn capacity_is_bounded() {
        let mut s = NoteStack::default();
        for n in 0..40 {
            s.note_on(0, n, 1);
        }
        assert_eq!(s.held().len(), MAX_HELD);
    }
}
