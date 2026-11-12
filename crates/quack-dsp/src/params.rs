//! Parameter registry: the single source of truth for ids, names,
//! ranges and defaults. The plugin adapter declares the same parameters
//! to the host (a test checks they match) and the editor reads ranges
//! from here.
//!
//! Host ids are permanent once a version ships (saved projects and
//! automation refer to them). Ranges reserved for the next steps:
//! `100..` pitch detection, `200..` pitch / formant shift, `300..`
//! autotune, `400..` harmonizer, `500..` vocoder, `600..` Manipulator
//! style effects.

/// How a parameter maps between plain and normalized (0..1) values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    /// Linear range.
    Float {
        min: f64,
        max: f64,
        default: f64,
    },
    Bool {
        default: bool,
    },
    /// Index into `count` choices.
    Enum {
        count: u32,
        default: u32,
    },
}

impl ParamKind {
    #[must_use]
    pub fn default_plain(self) -> f64 {
        match self {
            Self::Float { default, .. } => default,
            Self::Bool { default } => f64::from(u8::from(default)),
            Self::Enum { default, .. } => f64::from(default),
        }
    }

    #[must_use]
    pub fn min(self) -> f64 {
        match self {
            Self::Float { min, .. } => min,
            Self::Bool { .. } | Self::Enum { .. } => 0.0,
        }
    }

    #[must_use]
    pub fn max(self) -> f64 {
        match self {
            Self::Float { max, .. } => max,
            Self::Bool { .. } => 1.0,
            Self::Enum { count, .. } => f64::from(count.saturating_sub(1)),
        }
    }

    /// Clamp, and round the stepped kinds.
    #[must_use]
    pub fn sanitize(self, plain: f64) -> f64 {
        let v = if plain.is_finite() {
            plain
        } else {
            self.default_plain()
        };
        let v = v.clamp(self.min(), self.max());
        match self {
            Self::Float { .. } => v,
            Self::Bool { .. } | Self::Enum { .. } => v.round(),
        }
    }

    #[must_use]
    pub fn normalize(self, plain: f64) -> f64 {
        let span = self.max() - self.min();
        if span <= 0.0 {
            return 0.0;
        }
        ((self.sanitize(plain) - self.min()) / span).clamp(0.0, 1.0)
    }

    #[must_use]
    pub fn denormalize(self, norm: f64) -> f64 {
        self.sanitize(self.min() + norm.clamp(0.0, 1.0) * (self.max() - self.min()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// Stable text key (presets, tests).
    pub key: &'static str,
    /// Host parameter id (permanent once shipped).
    pub host_id: u32,
    /// Name shown by the host.
    pub name: &'static str,
    pub unit: &'static str,
    pub kind: ParamKind,
}

/// The engine's parameters, in host order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ParamId {
    InputGain,
    OutputGain,
    Mix,
    Mode,
    Bypass,
    Pitch,
    Fine,
    Formant,
    Correction,
    Retune,
    Humanize,
    Vibrato,
    Key,
    Scale,
    Source,
    NoteC,
    NoteCs,
    NoteD,
    NoteDs,
    NoteE,
    NoteF,
    NoteFs,
    NoteG,
    NoteGs,
    NoteA,
    NoteAs,
    NoteB,
    MidiGate,
    Transition,
    FormantFollow,
    OctaveDown,
    Sub,
    OctaveUp,
    Freeze,
    Robot,
    Glitch,
    GlitchRate,
    GlitchMode,
    GlitchHold,
    Voice,
    LowLatency,
    Harmony,
    Harmony1,
    Harmony2,
    Harmony3,
    Harmony4,
    Lead,
    HarmonyLevel,
    Spread,
    HarmonyHumanize,
    Character,
    Expression,
    Glide,
    Double,
    CloseVoicing,
    Latch,
}

pub const PARAM_COUNT: usize = 56;

/// The twelve note switches of the autotune, C first.
pub const NOTE_PARAMS: [ParamId; 12] = [
    ParamId::NoteC,
    ParamId::NoteCs,
    ParamId::NoteD,
    ParamId::NoteDs,
    ParamId::NoteE,
    ParamId::NoteF,
    ParamId::NoteFs,
    ParamId::NoteG,
    ParamId::NoteGs,
    ParamId::NoteA,
    ParamId::NoteAs,
    ParamId::NoteB,
];

const SPECS: [ParamSpec; PARAM_COUNT] = [
    ParamSpec {
        key: "input_gain",
        host_id: 0,
        name: "Input Gain",
        unit: "dB",
        kind: ParamKind::Float {
            min: -24.0,
            max: 24.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "output_gain",
        host_id: 1,
        name: "Output Gain",
        unit: "dB",
        kind: ParamKind::Float {
            min: -24.0,
            max: 24.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "mix",
        host_id: 2,
        name: "Mix",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "mode",
        host_id: 3,
        name: "Mode",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::mode::VoiceMode::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "bypass",
        host_id: 4,
        name: "Bypass",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "pitch",
        host_id: 200,
        name: "Pitch",
        unit: "st",
        kind: ParamKind::Float {
            min: -24.0,
            max: 24.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "fine",
        host_id: 201,
        name: "Fine",
        unit: "ct",
        kind: ParamKind::Float {
            min: -100.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "formant",
        host_id: 202,
        name: "Formant",
        unit: "st",
        kind: ParamKind::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "correction",
        host_id: 300,
        name: "Correction",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "retune",
        host_id: 301,
        name: "Retune",
        unit: "ms",
        kind: ParamKind::Float {
            min: 0.0,
            max: 400.0,
            default: 50.0,
        },
    },
    ParamSpec {
        key: "humanize",
        host_id: 302,
        name: "Humanize",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 20.0,
        },
    },
    ParamSpec {
        key: "vibrato",
        host_id: 303,
        name: "Vibrato",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 80.0,
        },
    },
    ParamSpec {
        key: "key",
        host_id: 304,
        name: "Key",
        unit: "",
        kind: ParamKind::Enum {
            count: 12,
            default: 0,
        },
    },
    ParamSpec {
        key: "scale",
        host_id: 305,
        name: "Scale",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::autotune::Scale::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "source",
        host_id: 306,
        name: "Source",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::autotune::Source::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "note_c",
        host_id: 307,
        name: "Note C",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "note_cs",
        host_id: 308,
        name: "Note C#",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "note_d",
        host_id: 309,
        name: "Note D",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "note_ds",
        host_id: 310,
        name: "Note D#",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "note_e",
        host_id: 311,
        name: "Note E",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "note_f",
        host_id: 312,
        name: "Note F",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "note_fs",
        host_id: 313,
        name: "Note F#",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "note_g",
        host_id: 314,
        name: "Note G",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "note_gs",
        host_id: 315,
        name: "Note G#",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "note_a",
        host_id: 316,
        name: "Note A",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "note_as",
        host_id: 317,
        name: "Note A#",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "note_b",
        host_id: 318,
        name: "Note B",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "midi_gate",
        host_id: 319,
        name: "MIDI Gate",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "transition",
        host_id: 320,
        name: "Transition",
        unit: "ms",
        kind: ParamKind::Float {
            min: 0.0,
            max: 300.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "formant_follow",
        host_id: 600,
        name: "Formant Follow",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "octave_down",
        host_id: 601,
        name: "Octave Down",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "sub",
        host_id: 602,
        name: "Sub",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "octave_up",
        host_id: 603,
        name: "Octave Up",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "freeze",
        host_id: 604,
        name: "Freeze",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "robot",
        host_id: 605,
        name: "Robot",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "glitch",
        host_id: 606,
        name: "Glitch",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "glitch_rate",
        host_id: 607,
        name: "Glitch Rate",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::creative::GlitchRate::COUNT,
            default: 2,
        },
    },
    ParamSpec {
        key: "glitch_mode",
        host_id: 608,
        name: "Glitch Mode",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::creative::GlitchMode::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "glitch_hold",
        host_id: 609,
        name: "Glitch Hold",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "voice",
        host_id: 100,
        name: "Voice",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::latency::VoiceRange::COUNT,
            default: 1,
        },
    },
    ParamSpec {
        key: "low_latency",
        host_id: 101,
        name: "Low Latency",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "harmony",
        host_id: 400,
        name: "Harmony",
        unit: "",
        kind: ParamKind::Enum {
            count: 4,
            default: 2,
        },
    },
    ParamSpec {
        key: "harmony_1",
        host_id: 401,
        name: "Harmony 1",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::harmony::INTERVAL_COUNT,
            default: (2 + crate::harmony::MAX_STEPS) as u32,
        },
    },
    ParamSpec {
        key: "harmony_2",
        host_id: 402,
        name: "Harmony 2",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::harmony::INTERVAL_COUNT,
            default: (-3 + crate::harmony::MAX_STEPS) as u32,
        },
    },
    ParamSpec {
        key: "harmony_3",
        host_id: 403,
        name: "Harmony 3",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::harmony::INTERVAL_COUNT,
            default: crate::harmony::MAX_STEPS as u32,
        },
    },
    ParamSpec {
        key: "harmony_4",
        host_id: 404,
        name: "Harmony 4",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::harmony::INTERVAL_COUNT,
            default: crate::harmony::MAX_STEPS as u32,
        },
    },
    ParamSpec {
        key: "lead",
        host_id: 405,
        name: "Lead",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "harmony_level",
        host_id: 406,
        name: "Harmony Level",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 80.0,
        },
    },
    ParamSpec {
        key: "spread",
        host_id: 407,
        name: "Spread",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 60.0,
        },
    },
    ParamSpec {
        key: "harmony_humanize",
        host_id: 408,
        name: "Harmony Humanize",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 30.0,
        },
    },
    ParamSpec {
        key: "character",
        host_id: 409,
        name: "Character",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 25.0,
        },
    },
    ParamSpec {
        key: "expression",
        host_id: 410,
        name: "Expression",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "glide",
        host_id: 411,
        name: "Glide",
        unit: "ms",
        kind: ParamKind::Float {
            min: 0.0,
            max: 500.0,
            default: 30.0,
        },
    },
    ParamSpec {
        key: "double",
        host_id: 412,
        name: "Double",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "close_voicing",
        host_id: 413,
        name: "Close Voicing",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "latch",
        host_id: 414,
        name: "Latch",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
];

impl ParamId {
    pub const ALL: [Self; PARAM_COUNT] = [
        Self::InputGain,
        Self::OutputGain,
        Self::Mix,
        Self::Mode,
        Self::Bypass,
        Self::Pitch,
        Self::Fine,
        Self::Formant,
        Self::Correction,
        Self::Retune,
        Self::Humanize,
        Self::Vibrato,
        Self::Key,
        Self::Scale,
        Self::Source,
        Self::NoteC,
        Self::NoteCs,
        Self::NoteD,
        Self::NoteDs,
        Self::NoteE,
        Self::NoteF,
        Self::NoteFs,
        Self::NoteG,
        Self::NoteGs,
        Self::NoteA,
        Self::NoteAs,
        Self::NoteB,
        Self::MidiGate,
        Self::Transition,
        Self::FormantFollow,
        Self::OctaveDown,
        Self::Sub,
        Self::OctaveUp,
        Self::Freeze,
        Self::Robot,
        Self::Glitch,
        Self::GlitchRate,
        Self::GlitchMode,
        Self::GlitchHold,
        Self::Voice,
        Self::LowLatency,
        Self::Harmony,
        Self::Harmony1,
        Self::Harmony2,
        Self::Harmony3,
        Self::Harmony4,
        Self::Lead,
        Self::HarmonyLevel,
        Self::Spread,
        Self::HarmonyHumanize,
        Self::Character,
        Self::Expression,
        Self::Glide,
        Self::Double,
        Self::CloseVoicing,
        Self::Latch,
    ];

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn spec(self) -> &'static ParamSpec {
        &SPECS[self as usize]
    }

    #[must_use]
    pub fn from_host_id(id: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.spec().host_id == id)
    }
}

/// Plain values of every parameter, indexed by [`ParamId::index`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamValues(pub [f64; PARAM_COUNT]);

impl Default for ParamValues {
    fn default() -> Self {
        Self(ParamId::ALL.map(|p| p.spec().kind.default_plain()))
    }
}

impl ParamValues {
    #[must_use]
    pub fn get(&self, id: ParamId) -> f64 {
        self.0[id.index()]
    }

    pub fn set(&mut self, id: ParamId, plain: f64) {
        self.0[id.index()] = id.spec().kind.sanitize(plain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_keys_are_unique_and_in_order() {
        for (i, p) in ParamId::ALL.iter().enumerate() {
            assert_eq!(p.index(), i);
            assert_eq!(ParamId::from_host_id(p.spec().host_id), Some(*p));
            for q in &ParamId::ALL[i + 1..] {
                assert_ne!(p.spec().key, q.spec().key);
            }
        }
    }

    #[test]
    fn normalize_round_trips_and_clamps() {
        for p in ParamId::ALL {
            let k = p.spec().kind;
            for n in [0.0, 0.25, 0.5, 1.0] {
                let plain = k.denormalize(n);
                assert!((k.denormalize(k.normalize(plain)) - plain).abs() < 1e-9);
            }
            assert_eq!(k.sanitize(f64::NAN), k.default_plain());
            assert_eq!(k.sanitize(1e9), k.max());
        }
    }
}
