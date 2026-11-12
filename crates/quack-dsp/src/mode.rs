//! Correction style: how the autotune corrects the lead voice.
//!
//! Quackapella used to have four exclusive modes; the creative effects,
//! harmonies and vocoder are now layers that play together (each with
//! its own switch and level), so what is left of the mode is the
//! character of the correction. It does not change the latency.

/// Correction style, from the most transparent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum VoiceMode {
    /// Correction and transposition that should not sound processed:
    /// slow and humanized retune, vibrato kept. The answer to
    /// "Manipulator sounds too artificial at times".
    #[default]
    Natural,
    /// Classic pitch correction, from gentle to the hard-tune effect.
    Tune,
}

impl VoiceMode {
    pub const COUNT: u32 = 2;
    pub const ALL: [Self; 2] = [Self::Natural, Self::Tune];

    /// From a parameter's plain value (index), clamped.
    #[must_use]
    pub fn from_index(i: f64) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (i.max(0.0).round() as usize).min(Self::ALL.len() - 1);
        Self::ALL[i]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Natural => "Natural",
            Self::Tune => "Tune",
        }
    }
}
