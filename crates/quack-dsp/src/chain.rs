//! Layer routing: parallel (every layer built from the voice and mixed)
//! or series (each module processes what the previous ones made).
//!
//! In series the voice stays at the head (it is the source) and the
//! three modules after it come in one of six orders. No module re-reads
//! the pitch: a second analysis would double the latency (about 18 ms
//! for the Soprano/Alto range), beyond the 10 ms budget for live use.
//! So:
//!
//! - The pitch effects (Creative's octave layers, robot, formant follow
//!   and freeze; the harmony voices) all ride on the voice's analysis
//!   grains. Pitch shifts commute, so building them all at the source
//!   is what a chain of shifters would sing.
//! - The glitch (Creative) cuts everything before it in the chain:
//!   placed after Harmony it cuts the harmonies too, after the Vocoder
//!   it cuts the vocoder.
//! - The vocoder's modulator is everything before it, and its level
//!   crossfades from what comes in (0 %) to the vocoded result alone
//!   (100 %), both at full level at 50 %.
//! - Harmony after the Vocoder: the vocoder's own pitch is its
//!   carrier's, so the internal carrier also plays the harmony notes.
//!   With the side-chain as carrier that cannot be done; the harmonies
//!   are then added to the vocoded signal as they are.

/// How the layers combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Routing {
    /// Every layer is built from the voice and they are mixed.
    #[default]
    Parallel,
    /// Each module processes the output of the previous one.
    Series,
}

impl Routing {
    pub const COUNT: u32 = 2;
    pub const ALL: [Self; 2] = [Self::Parallel, Self::Series];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        if i >= 0.5 {
            Self::Series
        } else {
            Self::Parallel
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Parallel => "Parallel",
            Self::Series => "Series",
        }
    }
}

/// A module of the chain after the voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Creative,
    Harmony,
    Vocoder,
}

impl Stage {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Creative => "Creative",
            Self::Harmony => "Harmony",
            Self::Vocoder => "Vocoder",
        }
    }
}

/// The order of the three modules after the voice, in series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chain(pub [Stage; 3]);

impl Default for Chain {
    fn default() -> Self {
        Self::ALL[0]
    }
}

impl Chain {
    pub const COUNT: u32 = 6;
    pub const ALL: [Self; 6] = {
        use Stage::{Creative as C, Harmony as H, Vocoder as V};
        [
            Self([C, H, V]),
            Self([H, C, V]),
            Self([C, V, H]),
            Self([H, V, C]),
            Self([V, C, H]),
            Self([V, H, C]),
        ]
    };
    pub const NAMES: [&'static str; 6] = [
        "Creative > Harmony > Vocoder",
        "Harmony > Creative > Vocoder",
        "Creative > Vocoder > Harmony",
        "Harmony > Vocoder > Creative",
        "Vocoder > Creative > Harmony",
        "Vocoder > Harmony > Creative",
    ];

    #[must_use]
    pub fn from_index(i: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (i.max(0.0).round() as usize).min(Self::ALL.len() - 1);
        Self::ALL[i]
    }

    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }

    /// Position of `stage` in the chain (0 to 2).
    #[must_use]
    pub fn position(self, stage: Stage) -> usize {
        self.0.iter().position(|s| *s == stage).unwrap_or(0)
    }

    /// Whether `a` comes before `b`.
    #[must_use]
    pub fn before(self, a: Stage, b: Stage) -> bool {
        self.position(a) < self.position(b)
    }

    /// The order with the module at `slot` (0 to 2) swapped with the
    /// next one.
    #[must_use]
    pub fn swapped(self, slot: usize) -> Self {
        let mut s = self.0;
        if slot + 1 < s.len() {
            s.swap(slot, slot + 1);
        }
        Self(s)
    }
}

/// Series crossfade of a module's level `w` (0 to 1): the gains of what
/// comes in and of the module's result. Both are at full level at 50 %.
#[must_use]
pub fn crossfade(w: f32) -> (f32, f32) {
    let w = w.clamp(0.0, 1.0);
    ((2.0 * (1.0 - w)).min(1.0), (2.0 * w).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_are_the_six_permutations() {
        for (i, c) in Chain::ALL.iter().enumerate() {
            assert_eq!(c.index(), i);
            assert_eq!(Chain::from_index(i as f64), *c);
            let name: Vec<&str> = c.0.iter().map(|s| s.name()).collect();
            assert_eq!(name.join(" > "), Chain::NAMES[i]);
            for (j, d) in Chain::ALL.iter().enumerate() {
                assert_eq!(i == j, c == d);
            }
        }
    }

    #[test]
    fn swapping_moves_one_module() {
        let c = Chain::default();
        assert_eq!(
            c.swapped(0).0,
            [Stage::Harmony, Stage::Creative, Stage::Vocoder]
        );
        assert_eq!(
            c.swapped(1).0,
            [Stage::Creative, Stage::Vocoder, Stage::Harmony]
        );
        assert_eq!(c.swapped(2), c);
    }

    #[test]
    fn crossfade_keeps_both_at_half() {
        assert_eq!(crossfade(0.0), (1.0, 0.0));
        assert_eq!(crossfade(0.5), (1.0, 1.0));
        assert_eq!(crossfade(1.0), (0.0, 1.0));
    }
}
