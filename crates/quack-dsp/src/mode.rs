//! Voice modes: how far the voice is pushed away from the singer.
//!
//! The mode selects defaults and algorithms for the steps to come; it
//! does not change the latency (constant across modes, see
//! [`crate::latency`]), so switching live never makes the host
//! re-align tracks.

/// Ordered from the most transparent to the most synthetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum VoiceMode {
    /// Correction and transposition that should not sound processed,
    /// even for large intervals: formants always preserved (spectral
    /// envelope kept in place), slow and humanized retune, vibrato and
    /// breath kept, noisy/unvoiced parts left untouched. This is the
    /// answer to "Manipulator sounds too artificial at times".
    #[default]
    Natural,
    /// Classic pitch correction, from gentle to the hard-tune effect.
    Tune,
    /// Manipulator-style play: pitch and formants decoupled, monster or
    /// chipmunk voices, freeze, glitch, octaves.
    Creative,
    /// VocalSynth-style: harmonies played over MIDI, vocoder, talkbox,
    /// synthetic voice.
    Synth,
}

impl VoiceMode {
    pub const COUNT: u32 = 4;
    pub const ALL: [Self; 4] = [Self::Natural, Self::Tune, Self::Creative, Self::Synth];

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
            Self::Creative => "Creative",
            Self::Synth => "Synth",
        }
    }

    /// Whether the spectral envelope (formants) is kept in place when
    /// the pitch moves. Only the creative modes may let it move.
    #[must_use]
    pub const fn preserves_formants(self) -> bool {
        matches!(self, Self::Natural | Self::Tune)
    }
}
