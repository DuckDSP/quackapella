//! Snapshots A and B, and the morph between them.
//!
//! A snapshot holds the values of every sound parameter (the ones a
//! preset sets, minus the macros). With Morph On, the Morph knob (0 % =
//! A, 100 % = B) writes the parameters themselves: the editor, the saved
//! state and the engine all follow, and the engine's own smoothing makes
//! each move glide. Continuous parameters are interpolated, switches and
//! choices flip at 50 %.
//!
//! Only the parameters that differ between A and B are written, and only
//! when the morph moves: a control A and B share stays free to tweak by
//! hand, and a tweak holds until the morph moves again.
//!
//! The morph can be driven by hand, by the host's automation, by a
//! macro, or by two MIDI notes ([`SnapshotKeys`]): the first glides to A,
//! the second to B, over Morph Time. Those two notes go no further (the
//! harmonizer and the autotune do not see them). With Morph Off, the
//! notes and the editor's A/B buttons recall a snapshot at once.

use crate::params::{MACRO_PARAMS, ParamId, ParamKind, ParamValues};
use crate::presets::in_preset;

/// Whether a snapshot holds this parameter.
#[must_use]
pub fn in_snapshot(id: ParamId) -> bool {
    in_preset(id) && !MACRO_PARAMS.contains(&id)
}

/// Value of `id` at morph position `t` (0 = A, 1 = B).
#[must_use]
pub fn morph_value(id: ParamId, a: &ParamValues, b: &ParamValues, t: f64) -> f64 {
    let (va, vb) = (a.get(id), b.get(id));
    match id.spec().kind {
        ParamKind::Float { .. } => va + (vb - va) * t.clamp(0.0, 1.0),
        ParamKind::Bool { .. } | ParamKind::Enum { .. } => {
            if t < 0.5 {
                va
            } else {
                vb
            }
        }
    }
}

/// The two MIDI notes that recall A and B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapshotKeys {
    #[default]
    Off,
    /// C-1 / C#-1 (notes 0 and 1).
    Cm1,
    /// C0 / C#0 (12, 13).
    C0,
    /// C1 / C#1 (24, 25).
    C1,
    /// C7 / C#7 (96, 97).
    C7,
    /// C8 / C#8 (108, 109).
    C8,
}

impl SnapshotKeys {
    pub const COUNT: u32 = 6;
    pub const ALL: [Self; 6] = [Self::Off, Self::Cm1, Self::C0, Self::C1, Self::C7, Self::C8];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = i.max(0.0).round() as usize;
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    /// Note of snapshot A (B is the next one up).
    #[must_use]
    pub const fn base(self) -> Option<u8> {
        match self {
            Self::Off => None,
            Self::Cm1 => Some(0),
            Self::C0 => Some(12),
            Self::C1 => Some(24),
            Self::C7 => Some(96),
            Self::C8 => Some(108),
        }
    }

    /// Snapshot recalled by `note` (0 = A, 1 = B).
    #[must_use]
    pub fn slot(self, note: u8) -> Option<usize> {
        let base = self.base()?;
        note.checked_sub(base).filter(|&d| d < 2).map(usize::from)
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Cm1 => "C-1 / C#-1",
            Self::C0 => "C0 / C#0",
            Self::C1 => "C1 / C#1",
            Self::C7 => "C7 / C#7",
            Self::C8 => "C8 / C#8",
        }
    }
}

/// A linear move of the Morph knob, advanced block by block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glide {
    from: f64,
    to: f64,
    len: f64,
    pos: f64,
}

impl Glide {
    /// From `from` to `to` (both in %) over `samples` (0 = a jump).
    #[must_use]
    pub fn new(from: f64, to: f64, samples: f64) -> Self {
        Self {
            from,
            to,
            len: samples.max(0.0),
            pos: 0.0,
        }
    }

    /// Advance by `frames`: the value at the end of the block.
    pub fn advance(&mut self, frames: usize) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        {
            self.pos += frames as f64;
        }
        if self.done() {
            return self.to;
        }
        self.from + (self.to - self.from) * (self.pos / self.len)
    }

    #[must_use]
    pub fn done(&self) -> bool {
        self.pos >= self.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_interpolate_and_switches_flip_halfway() {
        let mut a = ParamValues::default();
        let mut b = ParamValues::default();
        a.set(ParamId::Pitch, -12.0);
        b.set(ParamId::Pitch, 12.0);
        a.set(ParamId::HarmonyOn, 0.0);
        b.set(ParamId::HarmonyOn, 1.0);
        assert!((morph_value(ParamId::Pitch, &a, &b, 0.25) + 6.0).abs() < 1e-9);
        assert_eq!(morph_value(ParamId::HarmonyOn, &a, &b, 0.49), 0.0);
        assert_eq!(morph_value(ParamId::HarmonyOn, &a, &b, 0.5), 1.0);
    }

    #[test]
    fn snapshots_hold_the_sound_not_the_setup() {
        assert!(in_snapshot(ParamId::Pitch));
        assert!(in_snapshot(ParamId::Routing));
        for id in [
            ParamId::Bypass,
            ParamId::Voice,
            ParamId::Macro1,
            ParamId::Morph,
            ParamId::MorphOn,
            ParamId::MidiOut,
        ] {
            assert!(!in_snapshot(id), "{id:?}");
        }
    }

    #[test]
    fn keys_map_two_notes() {
        let k = SnapshotKeys::C0;
        assert_eq!(k.slot(12), Some(0));
        assert_eq!(k.slot(13), Some(1));
        assert_eq!(k.slot(14), None);
        assert_eq!(k.slot(11), None);
        assert_eq!(SnapshotKeys::Off.slot(0), None);
        for (i, k) in SnapshotKeys::ALL.into_iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let i = i as f64;
            assert_eq!(SnapshotKeys::from_index(i), k);
        }
    }

    #[test]
    fn glide_reaches_its_target() {
        let mut g = Glide::new(0.0, 100.0, 1000.0);
        assert!((g.advance(250) - 25.0).abs() < 1e-9);
        assert!(!g.done());
        assert_eq!(g.advance(1000), 100.0);
        assert!(g.done());
        let mut j = Glide::new(30.0, 0.0, 0.0);
        assert_eq!(j.advance(1), 0.0);
    }
}
