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

/// Held notes in press order (oldest first), with the sustain pedal
/// (CC 64) and a latch: a note released while either holds stays in the
/// stack until the pedal is lifted, or (latch) until the next chord is
/// played on released keys.
#[derive(Debug, Clone, Copy)]
pub struct NoteStack {
    notes: [HeldNote; MAX_HELD],
    len: usize,
    /// Keys physically down, and notes released but still held.
    down: u128,
    pending: u128,
    sustain: bool,
    latch: bool,
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
            down: 0,
            pending: 0,
            sustain: false,
            latch: false,
        }
    }
}

impl NoteStack {
    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8) {
        let note = note.min(127);
        // Latch: a chord played on released keys replaces the held one.
        if self.latch && self.down == 0 {
            self.len = 0;
            self.pending = 0;
        }
        self.down |= 1 << note;
        self.pending &= !(1u128 << note);
        // A repeated press moves the note to the top.
        self.remove(channel, note);
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
        let note = note.min(127);
        self.down &= !(1u128 << note);
        if (self.sustain || self.latch) && self.held().iter().any(|n| n.note == note) {
            self.pending |= 1 << note;
            return;
        }
        self.remove(channel, note);
    }

    /// Sustain pedal down or up (CC 64): lifting it releases the notes
    /// whose keys are up.
    pub fn set_sustain(&mut self, on: bool) {
        self.sustain = on;
        self.release_pending();
    }

    /// Latch on or off: off releases the notes whose keys are up.
    pub fn set_latch(&mut self, on: bool) {
        self.latch = on;
        self.release_pending();
    }

    fn release_pending(&mut self) {
        if self.sustain || self.latch || self.pending == 0 {
            return;
        }
        let mut i = 0;
        while i < self.len {
            if self.pending & (1u128 << self.notes[i].note) != 0 {
                self.notes.copy_within(i + 1..self.len, i);
                self.len -= 1;
            } else {
                i += 1;
            }
        }
        self.pending = 0;
    }

    fn remove(&mut self, channel: u8, note: u8) {
        if let Some(i) = self
            .held()
            .iter()
            .position(|n| n.note == note && n.channel == channel)
        {
            self.notes.copy_within(i + 1..self.len, i);
            self.len -= 1;
        }
    }

    /// All notes off (CC 120 / 123, reset), the pedal and latch kept.
    pub fn clear(&mut self) {
        self.len = 0;
        self.down = 0;
        self.pending = 0;
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
    fn sustain_and_latch_hold_released_notes() {
        let mut s = NoteStack::default();
        s.set_sustain(true);
        s.note_on(0, 60, 100);
        s.note_off(0, 60);
        assert_eq!(s.held().len(), 1);
        s.note_on(0, 64, 100);
        s.set_sustain(false);
        // 60 released, 64 still down.
        assert_eq!(s.mask(), 1 << 64);
        s.note_off(0, 64);
        assert!(s.held().is_empty());
        // Latch: the chord stays, the next one replaces it.
        s.set_latch(true);
        s.note_on(0, 60, 100);
        s.note_on(0, 64, 100);
        s.note_off(0, 60);
        s.note_off(0, 64);
        assert_eq!(s.mask(), (1 << 60) | (1 << 64));
        s.note_on(0, 62, 100);
        s.note_on(0, 65, 100);
        s.note_off(0, 62);
        assert_eq!(s.mask(), (1 << 62) | (1 << 65));
        s.set_latch(false);
        assert_eq!(s.mask(), 1 << 65);
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
