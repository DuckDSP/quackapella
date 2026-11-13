//! Parameter registry: the single source of truth for ids, names,
//! ranges and defaults. The plugin adapter declares the same parameters
//! to the host (a test checks they match) and the editor reads ranges
//! from here.
//!
//! Host ids are permanent once a version ships (saved projects and
//! automation refer to them). Ranges reserved for the next steps:
//! `100..` pitch detection, `200..` pitch / formant shift, `300..`
//! autotune, `400..` harmonizer, `500..` vocoder, `600..` Manipulator
//! style effects, `700..` macros, `800..` layer routing, `900..` voice effects
//! (dynamics, delay, reverb), `1000..` vocoder enrichments, `1100..`
//! throat and harmony humanize, `1200..` snapshots and MIDI out, `1300..`
//! output EQ, exciter and (later) the mixing assistant.

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
    Vocoder,
    Carrier,
    Bands,
    VocoderLevel,
    Response,
    BandShift,
    Sibilance,
    CarrierOctave,
    Detune,
    CarrierNoise,
    CarrierRelease,
    CarrierFollow,
    CarrierGlide,
    VocoderHold,
    FollowChord,
    BendRange,
    VocoderWidth,
    VocoderLofi,
    CreativeOn,
    CreativeLevel,
    HarmonyOn,
    VocoderOn,
    HarmonyLevel1,
    HarmonyLevel2,
    HarmonyLevel3,
    HarmonyLevel4,
    Macro1,
    Macro2,
    Macro3,
    Macro4,
    Routing,
    Chain,
    Throat,
    ThroatPitch,
    HarmonyVibrato,
    HarmonyTiming,
    HarmonyPan1,
    HarmonyPan2,
    HarmonyPan3,
    HarmonyPan4,
    VocoderSwap,
    Consonants,
    VocoderResonance,
    VocoderTilt,
    VocoderGate,
    Ensemble,
    CarrierSub,
    FxOn,
    FxLevel,
    Gate,
    DeEss,
    DeEssFreq,
    CompThreshold,
    CompRatio,
    CompAttack,
    CompRelease,
    CompMakeup,
    DelaySync,
    DelayDivision,
    DelayTime,
    DelayFeedback,
    DelayPingPong,
    DelayTone,
    DelayLevel,
    ReverbDecay,
    ReverbDamping,
    ReverbPredelay,
    ReverbLevel,
    Duck,
    VoiceReverb,
    CreativeReverb,
    HarmonyReverb,
    VocoderReverb,
    Morph,
    MorphOn,
    MorphTime,
    SnapshotKeys,
    MidiOut,
    MidiOutChannel,
    MidiOutSource,
    DynamicsOn,
    ToneOn,
    DelayOn,
    ReverbOn,
    ChorusOn,
    LowCut,
    Presence,
    Air,
    Drive,
    DelayMod,
    DelayWidth,
    DelayLowCut,
    ReverbSize,
    ReverbMod,
    ReverbLowCut,
    ReverbWidth,
    DelayToReverb,
    ChorusRate,
    ChorusDepth,
    OutputOn,
    OutDeEss,
    OutComp,
    OutCeiling,
    KeepInKey,
    DriveStyle,
    DriveTone,
    DriveOn,
    VoiceChorus,
    CreativeChorus,
    HarmonyChorus,
    VocoderChorus,
    VoiceDelay,
    CreativeDelay,
    HarmonyDelay,
    VocoderDelay,
    VoiceDrive,
    CreativeDrive,
    HarmonyDrive,
    VocoderDrive,
    DelayDuck,
    EqOn,
    EqLowCut,
    EqLowShelfFreq,
    EqLowShelfGain,
    EqBell1Freq,
    EqBell1Gain,
    EqBell1Q,
    EqBell2Freq,
    EqBell2Gain,
    EqBell2Q,
    EqBell3Freq,
    EqBell3Gain,
    EqBell3Q,
    EqHighShelfFreq,
    EqHighShelfGain,
    ExciterAmount,
    ExciterFreq,
}

pub const PARAM_COUNT: usize = 193;

/// Stereo place of each harmony interval voice.
pub const HARMONY_PAN_PARAMS: [ParamId; 4] = [
    ParamId::HarmonyPan1,
    ParamId::HarmonyPan2,
    ParamId::HarmonyPan3,
    ParamId::HarmonyPan4,
];

/// The bells of the output EQ: frequency, gain, Q.
pub const EQ_BELL_PARAMS: [[ParamId; 3]; 3] = [
    [
        ParamId::EqBell1Freq,
        ParamId::EqBell1Gain,
        ParamId::EqBell1Q,
    ],
    [
        ParamId::EqBell2Freq,
        ParamId::EqBell2Gain,
        ParamId::EqBell2Q,
    ],
    [
        ParamId::EqBell3Freq,
        ParamId::EqBell3Gain,
        ParamId::EqBell3Q,
    ],
];

/// The four macro knobs (see [`crate::control`]).
pub const MACRO_PARAMS: [ParamId; 4] = [
    ParamId::Macro1,
    ParamId::Macro2,
    ParamId::Macro3,
    ParamId::Macro4,
];

/// Level of each harmony interval voice.
pub const HARMONY_LEVEL_PARAMS: [ParamId; 4] = [
    ParamId::HarmonyLevel1,
    ParamId::HarmonyLevel2,
    ParamId::HarmonyLevel3,
    ParamId::HarmonyLevel4,
];

/// Per layer (voice, creative, harmony, vocoder): reverb send, delay
/// send, chorus amount and drive amount.
pub const REVERB_SEND_PARAMS: [ParamId; 4] = [
    ParamId::VoiceReverb,
    ParamId::CreativeReverb,
    ParamId::HarmonyReverb,
    ParamId::VocoderReverb,
];
pub const DELAY_SEND_PARAMS: [ParamId; 4] = [
    ParamId::VoiceDelay,
    ParamId::CreativeDelay,
    ParamId::HarmonyDelay,
    ParamId::VocoderDelay,
];
pub const CHORUS_AMOUNT_PARAMS: [ParamId; 4] = [
    ParamId::VoiceChorus,
    ParamId::CreativeChorus,
    ParamId::HarmonyChorus,
    ParamId::VocoderChorus,
];
pub const DRIVE_AMOUNT_PARAMS: [ParamId; 4] = [
    ParamId::VoiceDrive,
    ParamId::CreativeDrive,
    ParamId::HarmonyDrive,
    ParamId::VocoderDrive,
];

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

static SPECS: [ParamSpec; PARAM_COUNT] = [
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
        name: "Style",
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
            default: (4 + crate::harmony::MAX_SEMIS) as u32,
        },
    },
    ParamSpec {
        key: "harmony_2",
        host_id: 402,
        name: "Harmony 2",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::harmony::INTERVAL_COUNT,
            default: (-5 + crate::harmony::MAX_SEMIS) as u32,
        },
    },
    ParamSpec {
        key: "harmony_3",
        host_id: 403,
        name: "Harmony 3",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::harmony::INTERVAL_COUNT,
            default: crate::harmony::MAX_SEMIS as u32,
        },
    },
    ParamSpec {
        key: "harmony_4",
        host_id: 404,
        name: "Harmony 4",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::harmony::INTERVAL_COUNT,
            default: crate::harmony::MAX_SEMIS as u32,
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
    ParamSpec {
        key: "vocoder",
        host_id: 500,
        name: "Vocoder",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::vocoder::VocoderMode::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "carrier",
        host_id: 501,
        name: "Carrier",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::vocoder::Carrier::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "bands",
        host_id: 502,
        name: "Bands",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::vocoder::BAND_CHOICES.len() as u32,
            default: 2,
        },
    },
    ParamSpec {
        key: "vocoder_level",
        host_id: 503,
        name: "Vocoder Level",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 80.0,
        },
    },
    ParamSpec {
        key: "response",
        host_id: 504,
        name: "Response",
        unit: "ms",
        kind: ParamKind::Float {
            min: 2.0,
            max: 100.0,
            default: 12.0,
        },
    },
    ParamSpec {
        key: "band_shift",
        host_id: 505,
        name: "Band Shift",
        unit: "st",
        kind: ParamKind::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "sibilance",
        host_id: 506,
        name: "Sibilance",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 50.0,
        },
    },
    ParamSpec {
        key: "carrier_octave",
        host_id: 507,
        name: "Carrier Octave",
        unit: "",
        kind: ParamKind::Enum {
            count: 5,
            default: 2,
        },
    },
    ParamSpec {
        key: "detune",
        host_id: 508,
        name: "Detune",
        unit: "ct",
        kind: ParamKind::Float {
            min: 0.0,
            max: 50.0,
            default: 12.0,
        },
    },
    ParamSpec {
        key: "carrier_noise",
        host_id: 509,
        name: "Carrier Noise",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 5.0,
        },
    },
    ParamSpec {
        key: "carrier_release",
        host_id: 510,
        name: "Carrier Release",
        unit: "ms",
        kind: ParamKind::Float {
            min: 5.0,
            max: 1000.0,
            default: 80.0,
        },
    },
    ParamSpec {
        key: "carrier_follow",
        host_id: 511,
        name: "Carrier Follow",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "carrier_glide",
        host_id: 512,
        name: "Carrier Glide",
        unit: "ms",
        kind: ParamKind::Float {
            min: 0.0,
            max: 500.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "vocoder_hold",
        host_id: 513,
        name: "Vocoder Hold",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "follow_chord",
        host_id: 514,
        name: "Follow Chord",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::vocoder::FollowChord::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "bend_range",
        host_id: 515,
        name: "Bend Range",
        unit: "st",
        kind: ParamKind::Float {
            min: 0.0,
            max: 12.0,
            default: 2.0,
        },
    },
    ParamSpec {
        key: "vocoder_width",
        host_id: 516,
        name: "Vocoder Width",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 50.0,
        },
    },
    ParamSpec {
        key: "vocoder_lofi",
        host_id: 517,
        name: "Vocoder Lo-Fi",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "creative_on",
        host_id: 610,
        name: "Creative",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "creative_level",
        host_id: 611,
        name: "Creative Level",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "harmony_on",
        host_id: 415,
        name: "Harmony On",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "vocoder_on",
        host_id: 518,
        name: "Vocoder On",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "harmony_1_level",
        host_id: 416,
        name: "Harmony 1 Level",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "harmony_2_level",
        host_id: 417,
        name: "Harmony 2 Level",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "harmony_3_level",
        host_id: 418,
        name: "Harmony 3 Level",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "harmony_4_level",
        host_id: 419,
        name: "Harmony 4 Level",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "macro_1",
        host_id: 700,
        name: "Macro 1",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "macro_2",
        host_id: 701,
        name: "Macro 2",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "macro_3",
        host_id: 702,
        name: "Macro 3",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "macro_4",
        host_id: 703,
        name: "Macro 4",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "routing",
        host_id: 800,
        name: "Routing",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::chain::Routing::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "chain",
        host_id: 801,
        name: "Chain Order",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::chain::Chain::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "throat",
        host_id: 1100,
        name: "Throat",
        unit: "%",
        kind: ParamKind::Float {
            min: -100.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "throat_pitch",
        host_id: 1101,
        name: "Throat Pitch",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "harmony_vibrato",
        host_id: 1110,
        name: "Harmony Vibrato",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "harmony_timing",
        host_id: 1111,
        name: "Harmony Timing",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "harmony_pan_1",
        host_id: 1120,
        name: "Harmony 1 Pan",
        unit: "%",
        kind: ParamKind::Float {
            min: -100.0,
            max: 100.0,
            default: -100.0,
        },
    },
    ParamSpec {
        key: "harmony_pan_2",
        host_id: 1121,
        name: "Harmony 2 Pan",
        unit: "%",
        kind: ParamKind::Float {
            min: -100.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "harmony_pan_3",
        host_id: 1122,
        name: "Harmony 3 Pan",
        unit: "%",
        kind: ParamKind::Float {
            min: -100.0,
            max: 100.0,
            default: -50.0,
        },
    },
    ParamSpec {
        key: "harmony_pan_4",
        host_id: 1123,
        name: "Harmony 4 Pan",
        unit: "%",
        kind: ParamKind::Float {
            min: -100.0,
            max: 100.0,
            default: 50.0,
        },
    },
    ParamSpec {
        key: "vocoder_swap",
        host_id: 1000,
        name: "Voice from Sidechain",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "consonants",
        host_id: 1001,
        name: "Consonants",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 30.0,
        },
    },
    ParamSpec {
        key: "vocoder_resonance",
        host_id: 1002,
        name: "Vocoder Resonance",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 25.0,
        },
    },
    ParamSpec {
        key: "vocoder_tilt",
        host_id: 1003,
        name: "Vocoder Brightness",
        unit: "dB",
        kind: ParamKind::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "vocoder_gate",
        host_id: 1004,
        name: "Vocoder Gate",
        unit: "dB",
        kind: ParamKind::Float {
            min: -90.0,
            max: -20.0,
            default: -90.0,
        },
    },
    ParamSpec {
        key: "ensemble",
        host_id: 1005,
        name: "Ensemble",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "carrier_sub",
        host_id: 1006,
        name: "Carrier Sub",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "fx_on",
        host_id: 900,
        name: "FX On",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "fx_level",
        host_id: 901,
        name: "FX Level",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "gate",
        host_id: 910,
        name: "Gate",
        unit: "dB",
        kind: ParamKind::Float {
            min: -80.0,
            max: -20.0,
            default: -80.0,
        },
    },
    ParamSpec {
        key: "deess",
        host_id: 911,
        name: "De-Ess",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "deess_freq",
        host_id: 912,
        name: "De-Ess Freq",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 3000.0,
            max: 12000.0,
            default: 6500.0,
        },
    },
    ParamSpec {
        key: "comp_threshold",
        host_id: 913,
        name: "Comp Threshold",
        unit: "dB",
        kind: ParamKind::Float {
            min: -48.0,
            max: 0.0,
            default: -18.0,
        },
    },
    ParamSpec {
        key: "comp_ratio",
        host_id: 914,
        name: "Comp Ratio",
        unit: "",
        kind: ParamKind::Float {
            min: 1.0,
            max: 20.0,
            default: 1.0,
        },
    },
    ParamSpec {
        key: "comp_attack",
        host_id: 915,
        name: "Comp Attack",
        unit: "ms",
        kind: ParamKind::Float {
            min: 0.1,
            max: 50.0,
            default: 5.0,
        },
    },
    ParamSpec {
        key: "comp_release",
        host_id: 916,
        name: "Comp Release",
        unit: "ms",
        kind: ParamKind::Float {
            min: 10.0,
            max: 800.0,
            default: 120.0,
        },
    },
    ParamSpec {
        key: "comp_makeup",
        host_id: 917,
        name: "Comp Makeup",
        unit: "dB",
        kind: ParamKind::Float {
            min: 0.0,
            max: 24.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "delay_sync",
        host_id: 920,
        name: "Delay Sync",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "delay_division",
        host_id: 921,
        name: "Delay Division",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::fx::Division::COUNT,
            default: 4,
        },
    },
    ParamSpec {
        key: "delay_time",
        host_id: 922,
        name: "Delay Time",
        unit: "ms",
        kind: ParamKind::Float {
            min: 10.0,
            max: 2000.0,
            default: 375.0,
        },
    },
    ParamSpec {
        key: "delay_feedback",
        host_id: 923,
        name: "Delay Feedback",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 95.0,
            default: 35.0,
        },
    },
    ParamSpec {
        key: "delay_ping_pong",
        host_id: 924,
        name: "Delay Ping-Pong",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "delay_tone",
        host_id: 925,
        name: "Delay Tone",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 30.0,
        },
    },
    ParamSpec {
        key: "delay_level",
        host_id: 926,
        name: "Delay Level",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "reverb_decay",
        host_id: 930,
        name: "Reverb Decay",
        unit: "s",
        kind: ParamKind::Float {
            min: 0.2,
            max: 10.0,
            default: 2.0,
        },
    },
    ParamSpec {
        key: "reverb_damping",
        host_id: 931,
        name: "Reverb Damping",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 50.0,
        },
    },
    ParamSpec {
        key: "reverb_predelay",
        host_id: 932,
        name: "Reverb Pre-Delay",
        unit: "ms",
        kind: ParamKind::Float {
            min: 0.0,
            max: 200.0,
            default: 20.0,
        },
    },
    ParamSpec {
        key: "reverb_level",
        host_id: 933,
        name: "Reverb Level",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 60.0,
        },
    },
    ParamSpec {
        key: "duck",
        host_id: 940,
        name: "Reverb Duck",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "voice_reverb",
        host_id: 950,
        name: "Voice Reverb",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 15.0,
        },
    },
    ParamSpec {
        key: "creative_reverb",
        host_id: 951,
        name: "Creative Reverb",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 30.0,
        },
    },
    ParamSpec {
        key: "harmony_reverb",
        host_id: 952,
        name: "Harmony Reverb",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 40.0,
        },
    },
    ParamSpec {
        key: "vocoder_reverb",
        host_id: 953,
        name: "Vocoder Reverb",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 30.0,
        },
    },
    ParamSpec {
        key: "morph",
        host_id: 1200,
        name: "Morph",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "morph_on",
        host_id: 1201,
        name: "Morph On",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "morph_time",
        host_id: 1202,
        name: "Morph Time",
        unit: "ms",
        kind: ParamKind::Float {
            min: 0.0,
            max: 4000.0,
            default: 300.0,
        },
    },
    ParamSpec {
        key: "snapshot_keys",
        host_id: 1203,
        name: "Snapshot Keys",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::snapshot::SnapshotKeys::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "midi_out",
        host_id: 1210,
        name: "MIDI Out",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "midi_out_channel",
        host_id: 1211,
        name: "MIDI Out Channel",
        unit: "",
        kind: ParamKind::Enum {
            count: 16,
            default: 0,
        },
    },
    ParamSpec {
        key: "midi_out_source",
        host_id: 1212,
        name: "MIDI Out Source",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::midiout::OutSource::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "dynamics_on",
        host_id: 954,
        name: "Dynamics On",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "tone_on",
        host_id: 955,
        name: "Tone On",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "delay_on",
        host_id: 956,
        name: "Delay On",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "reverb_on",
        host_id: 957,
        name: "Reverb On",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "chorus_on",
        host_id: 958,
        name: "Chorus On",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "low_cut",
        host_id: 960,
        name: "Low Cut",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 20.0,
            max: 400.0,
            default: 20.0,
        },
    },
    ParamSpec {
        key: "presence",
        host_id: 961,
        name: "Presence",
        unit: "dB",
        kind: ParamKind::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "air",
        host_id: 962,
        name: "Air",
        unit: "dB",
        kind: ParamKind::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "drive",
        host_id: 963,
        name: "Drive",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 40.0,
        },
    },
    ParamSpec {
        key: "delay_mod",
        host_id: 970,
        name: "Delay Mod",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "delay_width",
        host_id: 971,
        name: "Delay Width",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "delay_low_cut",
        host_id: 972,
        name: "Delay Low Cut",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 20.0,
            max: 1000.0,
            default: 150.0,
        },
    },
    ParamSpec {
        key: "reverb_size",
        host_id: 975,
        name: "Reverb Size",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 50.0,
        },
    },
    ParamSpec {
        key: "reverb_mod",
        host_id: 976,
        name: "Reverb Mod",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 20.0,
        },
    },
    ParamSpec {
        key: "reverb_low_cut",
        host_id: 977,
        name: "Reverb Low Cut",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 20.0,
            max: 1000.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "reverb_width",
        host_id: 978,
        name: "Reverb Width",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "delay_to_reverb",
        host_id: 979,
        name: "Delay to Reverb",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "chorus_rate",
        host_id: 985,
        name: "Chorus Rate",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 0.1,
            max: 5.0,
            default: 0.8,
        },
    },
    ParamSpec {
        key: "chorus_depth",
        host_id: 986,
        name: "Chorus Depth",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 40.0,
        },
    },
    ParamSpec {
        key: "output_on",
        host_id: 941,
        name: "Output On",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "out_deess",
        host_id: 942,
        name: "Out De-Ess",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "out_comp",
        host_id: 943,
        name: "Out Comp",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "out_ceiling",
        host_id: 944,
        name: "Out Ceiling",
        unit: "dB",
        kind: ParamKind::Float {
            min: -12.0,
            max: 0.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "keep_in_key",
        host_id: 959,
        name: "Keep In Key",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "drive_style",
        host_id: 964,
        name: "Drive Style",
        unit: "",
        kind: ParamKind::Enum {
            count: crate::fx::DriveStyle::COUNT,
            default: 0,
        },
    },
    ParamSpec {
        key: "drive_tone",
        host_id: 965,
        name: "Drive Tone",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 60.0,
        },
    },
    ParamSpec {
        key: "drive_on",
        host_id: 966,
        name: "Drive On",
        unit: "",
        kind: ParamKind::Bool { default: false },
    },
    ParamSpec {
        key: "voice_chorus",
        host_id: 980,
        name: "Voice Chorus",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 50.0,
        },
    },
    ParamSpec {
        key: "creative_chorus",
        host_id: 981,
        name: "Creative Chorus",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 50.0,
        },
    },
    ParamSpec {
        key: "harmony_chorus",
        host_id: 982,
        name: "Harmony Chorus",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 50.0,
        },
    },
    ParamSpec {
        key: "vocoder_chorus",
        host_id: 983,
        name: "Vocoder Chorus",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 50.0,
        },
    },
    ParamSpec {
        key: "voice_delay",
        host_id: 990,
        name: "Voice Delay",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 15.0,
        },
    },
    ParamSpec {
        key: "creative_delay",
        host_id: 991,
        name: "Creative Delay",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 30.0,
        },
    },
    ParamSpec {
        key: "harmony_delay",
        host_id: 992,
        name: "Harmony Delay",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 40.0,
        },
    },
    ParamSpec {
        key: "vocoder_delay",
        host_id: 993,
        name: "Vocoder Delay",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 30.0,
        },
    },
    ParamSpec {
        key: "voice_drive",
        host_id: 994,
        name: "Voice Drive",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "creative_drive",
        host_id: 995,
        name: "Creative Drive",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "harmony_drive",
        host_id: 996,
        name: "Harmony Drive",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "vocoder_drive",
        host_id: 997,
        name: "Vocoder Drive",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 100.0,
        },
    },
    ParamSpec {
        key: "delay_duck",
        host_id: 945,
        name: "Delay Duck",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "eq_on",
        host_id: 1300,
        name: "EQ On",
        unit: "",
        kind: ParamKind::Bool { default: true },
    },
    ParamSpec {
        key: "eq_low_cut",
        host_id: 1301,
        name: "EQ Low Cut",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 20.0,
            max: 400.0,
            default: 20.0,
        },
    },
    ParamSpec {
        key: "eq_low_shelf_freq",
        host_id: 1302,
        name: "EQ Low Shelf Freq",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 40.0,
            max: 600.0,
            default: 150.0,
        },
    },
    ParamSpec {
        key: "eq_low_shelf_gain",
        host_id: 1303,
        name: "EQ Low Shelf Gain",
        unit: "dB",
        kind: ParamKind::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "eq_bell_1_freq",
        host_id: 1304,
        name: "EQ Bell 1 Freq",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 60.0,
            max: 16000.0,
            default: 300.0,
        },
    },
    ParamSpec {
        key: "eq_bell_1_gain",
        host_id: 1305,
        name: "EQ Bell 1 Gain",
        unit: "dB",
        kind: ParamKind::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "eq_bell_1_q",
        host_id: 1306,
        name: "EQ Bell 1 Q",
        unit: "",
        kind: ParamKind::Float {
            min: 0.3,
            max: 8.0,
            default: 1.0,
        },
    },
    ParamSpec {
        key: "eq_bell_2_freq",
        host_id: 1307,
        name: "EQ Bell 2 Freq",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 60.0,
            max: 16000.0,
            default: 1500.0,
        },
    },
    ParamSpec {
        key: "eq_bell_2_gain",
        host_id: 1308,
        name: "EQ Bell 2 Gain",
        unit: "dB",
        kind: ParamKind::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "eq_bell_2_q",
        host_id: 1309,
        name: "EQ Bell 2 Q",
        unit: "",
        kind: ParamKind::Float {
            min: 0.3,
            max: 8.0,
            default: 1.0,
        },
    },
    ParamSpec {
        key: "eq_bell_3_freq",
        host_id: 1310,
        name: "EQ Bell 3 Freq",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 60.0,
            max: 16000.0,
            default: 4000.0,
        },
    },
    ParamSpec {
        key: "eq_bell_3_gain",
        host_id: 1311,
        name: "EQ Bell 3 Gain",
        unit: "dB",
        kind: ParamKind::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "eq_bell_3_q",
        host_id: 1312,
        name: "EQ Bell 3 Q",
        unit: "",
        kind: ParamKind::Float {
            min: 0.3,
            max: 8.0,
            default: 1.0,
        },
    },
    ParamSpec {
        key: "eq_high_shelf_freq",
        host_id: 1313,
        name: "EQ High Shelf Freq",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 2000.0,
            max: 16000.0,
            default: 9000.0,
        },
    },
    ParamSpec {
        key: "eq_high_shelf_gain",
        host_id: 1314,
        name: "EQ High Shelf Gain",
        unit: "dB",
        kind: ParamKind::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "exciter_amount",
        host_id: 1315,
        name: "Exciter",
        unit: "%",
        kind: ParamKind::Float {
            min: 0.0,
            max: 100.0,
            default: 0.0,
        },
    },
    ParamSpec {
        key: "exciter_freq",
        host_id: 1316,
        name: "Exciter Freq",
        unit: "Hz",
        kind: ParamKind::Float {
            min: 1500.0,
            max: 8000.0,
            default: 3500.0,
        },
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
        Self::Vocoder,
        Self::Carrier,
        Self::Bands,
        Self::VocoderLevel,
        Self::Response,
        Self::BandShift,
        Self::Sibilance,
        Self::CarrierOctave,
        Self::Detune,
        Self::CarrierNoise,
        Self::CarrierRelease,
        Self::CarrierFollow,
        Self::CarrierGlide,
        Self::VocoderHold,
        Self::FollowChord,
        Self::BendRange,
        Self::VocoderWidth,
        Self::VocoderLofi,
        Self::CreativeOn,
        Self::CreativeLevel,
        Self::HarmonyOn,
        Self::VocoderOn,
        Self::HarmonyLevel1,
        Self::HarmonyLevel2,
        Self::HarmonyLevel3,
        Self::HarmonyLevel4,
        Self::Macro1,
        Self::Macro2,
        Self::Macro3,
        Self::Macro4,
        Self::Routing,
        Self::Chain,
        Self::Throat,
        Self::ThroatPitch,
        Self::HarmonyVibrato,
        Self::HarmonyTiming,
        Self::HarmonyPan1,
        Self::HarmonyPan2,
        Self::HarmonyPan3,
        Self::HarmonyPan4,
        Self::VocoderSwap,
        Self::Consonants,
        Self::VocoderResonance,
        Self::VocoderTilt,
        Self::VocoderGate,
        Self::Ensemble,
        Self::CarrierSub,
        Self::FxOn,
        Self::FxLevel,
        Self::Gate,
        Self::DeEss,
        Self::DeEssFreq,
        Self::CompThreshold,
        Self::CompRatio,
        Self::CompAttack,
        Self::CompRelease,
        Self::CompMakeup,
        Self::DelaySync,
        Self::DelayDivision,
        Self::DelayTime,
        Self::DelayFeedback,
        Self::DelayPingPong,
        Self::DelayTone,
        Self::DelayLevel,
        Self::ReverbDecay,
        Self::ReverbDamping,
        Self::ReverbPredelay,
        Self::ReverbLevel,
        Self::Duck,
        Self::VoiceReverb,
        Self::CreativeReverb,
        Self::HarmonyReverb,
        Self::VocoderReverb,
        Self::Morph,
        Self::MorphOn,
        Self::MorphTime,
        Self::SnapshotKeys,
        Self::MidiOut,
        Self::MidiOutChannel,
        Self::MidiOutSource,
        Self::DynamicsOn,
        Self::ToneOn,
        Self::DelayOn,
        Self::ReverbOn,
        Self::ChorusOn,
        Self::LowCut,
        Self::Presence,
        Self::Air,
        Self::Drive,
        Self::DelayMod,
        Self::DelayWidth,
        Self::DelayLowCut,
        Self::ReverbSize,
        Self::ReverbMod,
        Self::ReverbLowCut,
        Self::ReverbWidth,
        Self::DelayToReverb,
        Self::ChorusRate,
        Self::ChorusDepth,
        Self::OutputOn,
        Self::OutDeEss,
        Self::OutComp,
        Self::OutCeiling,
        Self::KeepInKey,
        Self::DriveStyle,
        Self::DriveTone,
        Self::DriveOn,
        Self::VoiceChorus,
        Self::CreativeChorus,
        Self::HarmonyChorus,
        Self::VocoderChorus,
        Self::VoiceDelay,
        Self::CreativeDelay,
        Self::HarmonyDelay,
        Self::VocoderDelay,
        Self::VoiceDrive,
        Self::CreativeDrive,
        Self::HarmonyDrive,
        Self::VocoderDrive,
        Self::DelayDuck,
        Self::EqOn,
        Self::EqLowCut,
        Self::EqLowShelfFreq,
        Self::EqLowShelfGain,
        Self::EqBell1Freq,
        Self::EqBell1Gain,
        Self::EqBell1Q,
        Self::EqBell2Freq,
        Self::EqBell2Gain,
        Self::EqBell2Q,
        Self::EqBell3Freq,
        Self::EqBell3Gain,
        Self::EqBell3Q,
        Self::EqHighShelfFreq,
        Self::EqHighShelfGain,
        Self::ExciterAmount,
        Self::ExciterFreq,
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
