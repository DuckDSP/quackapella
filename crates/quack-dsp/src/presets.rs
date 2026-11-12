//! Presets: the factory library (compiled in, so a MIDI program change
//! can recall one on the audio thread without allocating) and the text
//! format user presets are saved in.
//!
//! A preset holds every parameter except the ones that belong to the
//! singer's setup rather than to a sound ([`in_preset`]): bypass, voice
//! range, the low latency switch, input gain and key. Recalling a preset
//! leaves those where they are. It also holds the four macros (names and
//! targets); MIDI learn stays with the session.
//!
//! Text format, one `key = value` per line, parameters by their stable
//! registry key (`#` starts a comment):
//!
//! ```text
//! # Quackapella preset
//! name = Hard Tune
//! category = Tune
//! mode = 1
//! retune = 0
//! macro_1.name = Hardness
//! macro_1.targets = retune:-25, humanize:-40
//! ```
//!
//! Missing parameters take their default, unknown lines are skipped, so
//! files stay readable across versions.

use std::fmt::Write as _;

use crate::control::{
    ControlState, MACRO_COUNT, by_key, lines, macro_key, parse_targets, write_macros,
};
use crate::params::{ParamId, ParamKind, ParamValues};

/// File extension of user presets.
pub const EXTENSION: &str = "qkpreset";

/// Whether a preset sets this parameter.
#[must_use]
pub fn in_preset(id: ParamId) -> bool {
    !matches!(
        id,
        ParamId::Bypass
            | ParamId::Voice
            | ParamId::LowLatency
            | ParamId::InputGain
            | ParamId::Key
            | ParamId::Morph
            | ParamId::MorphOn
            | ParamId::MorphTime
            | ParamId::SnapshotKeys
            | ParamId::MidiOut
            | ParamId::MidiOutChannel
            | ParamId::MidiOutSource
    )
}

/// A factory macro: name and targets (depth as a fraction of the
/// target's range, -1 to 1).
#[derive(Debug, Clone, Copy)]
pub struct MacroDef {
    pub name: &'static str,
    pub targets: &'static [(ParamId, f64)],
}

#[derive(Debug, Clone, Copy)]
pub struct Factory {
    pub name: &'static str,
    pub category: &'static str,
    /// Values that differ from the defaults.
    pub values: &'static [(ParamId, f64)],
    pub macros: [MacroDef; MACRO_COUNT],
}

impl Factory {
    /// The preset's values (allocation free).
    #[must_use]
    pub fn values(&self) -> ParamValues {
        let mut v = ParamValues::default();
        for &(id, x) in self.values {
            v.set(id, x);
        }
        v
    }

    #[must_use]
    pub fn to_preset(&self) -> Preset {
        Preset {
            name: self.name.to_owned(),
            category: self.category.to_owned(),
            values: self.values(),
            macro_names: self.macros.map(|m| m.name.to_owned()),
            macros: self.macros.map(|m| m.targets.to_vec()),
        }
    }
}

/// Merge a preset's values into the current ones: the setup
/// parameters ([`in_preset`]) keep their current value.
#[must_use]
pub fn merge(current: &ParamValues, preset: &ParamValues) -> ParamValues {
    let mut v = *current;
    for id in ParamId::ALL {
        if in_preset(id) {
            v.set(id, preset.get(id));
        }
    }
    v
}

/// Whether `current` still sounds like `preset` (setup parameters
/// ignored), for the "modified" mark.
#[must_use]
pub fn matches(current: &ParamValues, preset: &ParamValues) -> bool {
    ParamId::ALL
        .into_iter()
        .filter(|id| in_preset(*id))
        .all(|id| {
            let tol = match id.spec().kind {
                ParamKind::Float { min, max, .. } => (max - min) * 1e-4,
                _ => 0.0,
            };
            (current.get(id) - preset.get(id)).abs() <= tol
        })
}

/// A preset in memory (user file or factory copy).
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub category: String,
    pub values: ParamValues,
    pub macro_names: [String; MACRO_COUNT],
    pub macros: [Vec<(ParamId, f64)>; MACRO_COUNT],
}

impl Preset {
    /// The current sound as a preset.
    #[must_use]
    pub fn capture(
        name: &str,
        category: &str,
        values: &ParamValues,
        control: &ControlState,
    ) -> Self {
        Self {
            name: name.trim().to_owned(),
            category: category.trim().to_owned(),
            values: *values,
            macro_names: control.names(),
            macros: std::array::from_fn(|m| {
                control
                    .targets(m)
                    .into_iter()
                    .map(|t| (t.id, f64::from(t.depth) / 100.0))
                    .collect()
            }),
        }
    }

    /// Recall the macros into `control` (editor thread).
    pub fn recall_macros(&self, control: &ControlState) {
        for m in 0..MACRO_COUNT {
            control.set_targets(m, &self.macros[m]);
            control.set_name(m, &self.macro_names[m]);
        }
    }

    #[must_use]
    pub fn to_text(&self) -> String {
        let mut s = String::from("# Quackapella preset\n");
        let _ = writeln!(s, "name = {}", one_line(&self.name));
        let _ = writeln!(s, "category = {}", one_line(&self.category));
        for id in ParamId::ALL.into_iter().filter(|id| in_preset(*id)) {
            let v = self.values.get(id);
            match id.spec().kind {
                ParamKind::Float { .. } => {
                    let _ = writeln!(s, "{} = {}", id.spec().key, round6(v));
                }
                _ => {
                    let _ = writeln!(s, "{} = {v:.0}", id.spec().key);
                }
            }
        }
        let names = self.macro_names.clone().map(|n| one_line(&n));
        write_macros(&mut s, &names, |m| {
            self.macros[m]
                .iter()
                .filter_map(|&(id, d)| {
                    #[allow(clippy::cast_possible_truncation)]
                    let depth = (d * 100.0).round().clamp(-100.0, 100.0) as i8;
                    (depth != 0).then_some(crate::control::Target { id, depth })
                })
                .collect()
        });
        s
    }

    /// Parse a preset file. `None` without a name line.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut p = Self {
            name: String::new(),
            category: String::new(),
            values: ParamValues::default(),
            macro_names: crate::control::DEFAULT_MACRO_NAMES.map(str::to_owned),
            macros: Default::default(),
        };
        let mut named = false;
        for (k, v) in lines(text) {
            match k {
                "name" => {
                    v.clone_into(&mut p.name);
                    named = true;
                }
                "category" => v.clone_into(&mut p.category),
                _ => {
                    if let Some((m, field)) = macro_key(k) {
                        match field {
                            "name" => v.clone_into(&mut p.macro_names[m]),
                            "targets" => p.macros[m] = parse_targets(v),
                            _ => {}
                        }
                    } else if let Some(id) = by_key(k)
                        && in_preset(id)
                        && let Ok(x) = v.parse::<f64>()
                    {
                        p.values.set(id, x);
                    }
                }
            }
        }
        named.then_some(p)
    }
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ").trim().to_owned()
}

fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

// ---- Factory library ------------------------------------------------

use ParamId as P;

// Enum indices, spelled out for the tables below.
const NATURAL: f64 = 0.0;
const TUNE: f64 = 1.0;
const ON: f64 = 1.0;
const MAJOR: f64 = 1.0;
const SRC_MIDI: f64 = 1.0;
const SRC_MIDI_EXACT: f64 = 2.0;
const H_MIDI: f64 = 0.0;
const H_SCALE: f64 = 2.0;
const H_CHORD: f64 = 3.0;
/// Harmony intervals: index = semitones + 24 (in the Scale and Chord
/// sources, a third is a third of the scale either way).
const IV_OFF: f64 = 24.0;
const IV_3RD_UP: f64 = 28.0;
const IV_5TH_UP: f64 = 31.0;
const IV_6TH_UP: f64 = 33.0;
const IV_OCT_UP: f64 = 36.0;
const IV_3RD_DOWN: f64 = 21.0;
const IV_4TH_DOWN: f64 = 19.0;
const IV_6TH_DOWN: f64 = 16.0;
const IV_OCT_DOWN: f64 = 12.0;
const VOCODER: f64 = 0.0;
const TALKBOX: f64 = 1.0;
const SAW: f64 = 0.0;
const SQUARE: f64 = 1.0;
const SUPERSAW: f64 = 2.0;
const NOISE: f64 = 3.0;
const BANDS_16: f64 = 1.0;
const BANDS_32: f64 = 3.0;
const GLITCH_STUTTER: f64 = 1.0;
const GLITCH_REVERSE: f64 = 2.0;
const GLITCH_TAPE: f64 = 3.0;
const RATE_8TH: f64 = 1.0;
const RATE_16TH: f64 = 2.0;
const OCT_DOWN_1: f64 = 1.0;

const fn mac(name: &'static str, targets: &'static [(ParamId, f64)]) -> MacroDef {
    MacroDef { name, targets }
}

const DRY_WET: MacroDef = mac("Dry / Wet", &[(P::Mix, -1.0)]);
const FORMANT: MacroDef = mac("Formant", &[(P::Formant, 0.25)]);

const NATURAL_MACROS: [MacroDef; MACRO_COUNT] = [
    mac(
        "Tightness",
        &[(P::Retune, -0.25), (P::Humanize, -0.3), (P::Vibrato, -0.4)],
    ),
    FORMANT,
    mac("Octave Up", &[(P::Pitch, 0.25)]),
    DRY_WET,
];

const TUNE_MACROS: [MacroDef; MACRO_COUNT] = [
    mac(
        "Hardness",
        &[
            (P::Retune, -0.25),
            (P::Humanize, -0.4),
            (P::Vibrato, -0.8),
            (P::Transition, -0.2),
        ],
    ),
    mac("Glide", &[(P::Transition, 0.4)]),
    FORMANT,
    DRY_WET,
];

const CREATIVE_MACROS: [MacroDef; MACRO_COUNT] = [
    mac(
        "Monster",
        &[(P::Pitch, -0.15), (P::Formant, -0.25), (P::OctaveDown, 0.5)],
    ),
    mac("Robot", &[(P::Robot, 1.0), (P::Retune, -0.25)]),
    mac("Glitch", &[(P::Glitch, 0.6)]),
    mac(
        "Octaves",
        &[(P::OctaveDown, 0.6), (P::Sub, 0.4), (P::OctaveUp, 0.4)],
    ),
];

const HARMONY_MACROS: [MacroDef; MACRO_COUNT] = [
    mac(
        "Choir",
        &[
            (P::HarmonyLevel, 0.2),
            (P::Spread, 0.4),
            (P::HarmonyHumanize, 0.4),
        ],
    ),
    mac("Double", &[(P::Double, 0.8)]),
    mac("Character", &[(P::Character, 0.6), (P::Expression, -0.4)]),
    mac("Lead", &[(P::Lead, -1.0)]),
];

const VOCODER_MACROS: [MacroDef; MACRO_COUNT] = [
    mac("Bright", &[(P::BandShift, 0.25), (P::Sibilance, 0.5)]),
    mac("Detune", &[(P::Detune, 0.6), (P::CarrierNoise, 0.2)]),
    mac(
        "Release",
        &[(P::CarrierRelease, 0.4), (P::CarrierGlide, 0.3)],
    ),
    mac("Voice Blend", &[(P::Lead, 0.6)]),
];

/// The vocoder layer on, the harmonies off.
/// Series routing and two of its orders (see [`crate::chain::Chain`]).
const SERIES: f64 = 1.0;
const CHAIN_CREATIVE_VOCODER_HARMONY: f64 = 2.0;
const CHAIN_HARMONY_VOCODER_CREATIVE: f64 = 3.0;

/// Voice effects: space, echo, squash, duck.
const FX_MACROS: [MacroDef; MACRO_COUNT] = [
    mac("Space", &[(P::SendVoice, 0.4), (P::ReverbLevel, 0.3)]),
    mac("Echo", &[(P::DelayLevel, 0.6), (P::DelayFeedback, 0.3)]),
    mac("Squash", &[(P::CompThreshold, -0.4), (P::CompMakeup, 0.2)]),
    mac("Duck", &[(P::Duck, 0.8)]),
];

const SYNTH_CLEAN: [(ParamId, f64); 3] =
    [(P::Mode, TUNE), (P::VocoderOn, ON), (P::Correction, 60.0)];

macro_rules! vals {
    ($($id:ident = $v:expr),* $(,)?) => { &[$((P::$id, $v)),*] };
}

/// The factory library, by category. Program change n recalls entry n.
pub const FACTORY: &[Factory] = &[
    // Natural: corrections and transpositions that should not show.
    Factory {
        name: "Init",
        category: "Natural",
        values: &[],
        macros: NATURAL_MACROS,
    },
    Factory {
        name: "Transparent Fix",
        category: "Natural",
        values: vals!(
            Mode = NATURAL,
            Correction = 70.0,
            Retune = 120.0,
            Humanize = 40.0,
            Vibrato = 100.0
        ),
        macros: NATURAL_MACROS,
    },
    Factory {
        name: "Octave Down",
        category: "Natural",
        values: vals!(Mode = NATURAL, Correction = 0.0, Pitch = -12.0),
        macros: NATURAL_MACROS,
    },
    Factory {
        name: "Fifth Up",
        category: "Natural",
        values: vals!(Mode = NATURAL, Correction = 0.0, Pitch = 7.0),
        macros: NATURAL_MACROS,
    },
    Factory {
        name: "Darker Voice",
        category: "Natural",
        values: vals!(Mode = NATURAL, Correction = 0.0, Formant = -2.5),
        macros: NATURAL_MACROS,
    },
    // Tune: from gentle to hard.
    Factory {
        name: "Gentle Tune",
        category: "Tune",
        values: vals!(
            Mode = TUNE,
            Correction = 80.0,
            Retune = 80.0,
            Humanize = 35.0,
            Vibrato = 90.0
        ),
        macros: TUNE_MACROS,
    },
    Factory {
        name: "Pop Tune",
        category: "Tune",
        values: vals!(
            Mode = TUNE,
            Scale = MAJOR,
            Retune = 25.0,
            Humanize = 15.0,
            Vibrato = 70.0
        ),
        macros: TUNE_MACROS,
    },
    Factory {
        name: "Hard Tune",
        category: "Tune",
        values: vals!(
            Mode = TUNE,
            Scale = MAJOR,
            Retune = 0.0,
            Humanize = 0.0,
            Vibrato = 0.0
        ),
        macros: TUNE_MACROS,
    },
    Factory {
        name: "Hard Tune Glide",
        category: "Tune",
        values: vals!(
            Mode = TUNE,
            Scale = MAJOR,
            Retune = 0.0,
            Humanize = 0.0,
            Vibrato = 0.0,
            Transition = 60.0
        ),
        macros: TUNE_MACROS,
    },
    Factory {
        name: "MIDI Tune",
        category: "Tune",
        values: vals!(
            Mode = TUNE,
            Source = SRC_MIDI,
            Retune = 10.0,
            Humanize = 10.0,
            Vibrato = 50.0
        ),
        macros: TUNE_MACROS,
    },
    Factory {
        name: "MIDI Exact Gate",
        category: "Tune",
        values: vals!(
            Mode = TUNE,
            Source = SRC_MIDI_EXACT,
            MidiGate = 1.0,
            Retune = 0.0,
            Humanize = 0.0,
            Vibrato = 0.0
        ),
        macros: TUNE_MACROS,
    },
    // Creative: pitch and formants apart, octaves, robot, glitch.
    Factory {
        name: "Monster",
        category: "Creative",
        values: vals!(
            Mode = TUNE,
            CreativeOn = ON,
            Correction = 0.0,
            Pitch = -7.0,
            Formant = -6.0,
            OctaveDown = 25.0
        ),
        macros: CREATIVE_MACROS,
    },
    Factory {
        name: "Chipmunk",
        category: "Creative",
        values: vals!(
            Mode = TUNE,
            CreativeOn = ON,
            Correction = 0.0,
            Pitch = 7.0,
            Formant = 6.0
        ),
        macros: CREATIVE_MACROS,
    },
    Factory {
        name: "Octave Stack",
        category: "Creative",
        values: vals!(
            Mode = TUNE,
            CreativeOn = ON,
            Retune = 20.0,
            OctaveDown = 60.0,
            Sub = 30.0,
            OctaveUp = 30.0
        ),
        macros: CREATIVE_MACROS,
    },
    Factory {
        name: "Robot",
        category: "Creative",
        values: vals!(
            Mode = TUNE,
            CreativeOn = ON,
            Lead = 0.0,
            Retune = 0.0,
            Humanize = 0.0,
            Vibrato = 0.0,
            Robot = 85.0
        ),
        macros: CREATIVE_MACROS,
    },
    Factory {
        name: "Stutter 1/16",
        category: "Creative",
        values: vals!(
            Mode = TUNE,
            CreativeOn = ON,
            Lead = 0.0,
            Correction = 0.0,
            Glitch = 30.0,
            GlitchMode = GLITCH_STUTTER,
            GlitchRate = RATE_16TH
        ),
        macros: CREATIVE_MACROS,
    },
    Factory {
        name: "Tape Stops",
        category: "Creative",
        values: vals!(
            Mode = TUNE,
            CreativeOn = ON,
            Lead = 0.0,
            Correction = 0.0,
            Glitch = 25.0,
            GlitchMode = GLITCH_TAPE,
            GlitchRate = RATE_8TH
        ),
        macros: CREATIVE_MACROS,
    },
    Factory {
        name: "Reverse Bits",
        category: "Creative",
        values: vals!(
            Mode = TUNE,
            CreativeOn = ON,
            Lead = 0.0,
            Correction = 0.0,
            Glitch = 25.0,
            GlitchMode = GLITCH_REVERSE,
            GlitchRate = RATE_8TH
        ),
        macros: CREATIVE_MACROS,
    },
    Factory {
        name: "Freeze Over MIDI",
        category: "Creative",
        values: vals!(
            Mode = TUNE,
            CreativeOn = ON,
            Lead = 0.0,
            Source = SRC_MIDI,
            Retune = 0.0,
            FormantFollow = 50.0
        ),
        macros: CREATIVE_MACROS,
    },
    // Harmonizer layer.
    Factory {
        name: "Third Above",
        category: "Harmony",
        values: vals!(
            Mode = TUNE,
            HarmonyOn = ON,
            Scale = MAJOR,
            Harmony = H_SCALE,
            Harmony1 = IV_3RD_UP,
            Harmony2 = IV_OFF,
            Harmony3 = IV_OFF,
            Harmony4 = IV_OFF,
            HarmonyLevel = 75.0
        ),
        macros: HARMONY_MACROS,
    },
    Factory {
        name: "Thirds and Sixths",
        category: "Harmony",
        values: vals!(
            Mode = TUNE,
            HarmonyOn = ON,
            Scale = MAJOR,
            Harmony = H_SCALE,
            Harmony1 = IV_3RD_UP,
            Harmony2 = IV_6TH_DOWN,
            Harmony3 = IV_OFF,
            Harmony4 = IV_OFF
        ),
        macros: HARMONY_MACROS,
    },
    Factory {
        name: "Four-Part Choir",
        category: "Harmony",
        values: vals!(
            Mode = TUNE,
            HarmonyOn = ON,
            Scale = MAJOR,
            Harmony = H_SCALE,
            Harmony1 = IV_3RD_UP,
            Harmony2 = IV_5TH_UP,
            Harmony3 = IV_OCT_DOWN,
            Harmony4 = IV_3RD_DOWN,
            Spread = 85.0,
            HarmonyHumanize = 50.0
        ),
        macros: HARMONY_MACROS,
    },
    Factory {
        name: "Power Fifths",
        category: "Harmony",
        values: vals!(
            Mode = TUNE,
            HarmonyOn = ON,
            Scale = MAJOR,
            Harmony = H_SCALE,
            Harmony1 = IV_5TH_UP,
            Harmony2 = IV_OCT_UP,
            Harmony3 = IV_4TH_DOWN,
            Harmony4 = IV_OFF,
            Character = 40.0
        ),
        macros: HARMONY_MACROS,
    },
    Factory {
        name: "MIDI Chords",
        category: "Harmony",
        values: vals!(
            Mode = TUNE,
            HarmonyOn = ON,
            Harmony = H_MIDI,
            HarmonyLevel = 80.0,
            Glide = 40.0
        ),
        macros: HARMONY_MACROS,
    },
    Factory {
        name: "Chord Follower",
        category: "Harmony",
        values: vals!(
            Mode = TUNE,
            HarmonyOn = ON,
            Harmony = H_CHORD,
            Harmony1 = IV_3RD_UP,
            Harmony2 = IV_6TH_UP,
            Harmony3 = IV_OFF,
            Harmony4 = IV_OFF,
            CloseVoicing = 1.0,
            Latch = 1.0
        ),
        macros: HARMONY_MACROS,
    },
    Factory {
        name: "Doubler",
        category: "Harmony",
        values: vals!(
            Mode = TUNE,
            HarmonyOn = ON,
            Harmony1 = IV_OFF,
            Harmony2 = IV_OFF,
            Double = 70.0,
            Correction = 60.0
        ),
        macros: HARMONY_MACROS,
    },
    // Vocoder and talkbox layer.
    Factory {
        name: "Classic Vocoder",
        category: "Vocoder",
        values: &[
            SYNTH_CLEAN[0],
            SYNTH_CLEAN[1],
            SYNTH_CLEAN[2],
            (P::Vocoder, VOCODER),
            (P::Carrier, SAW),
            (P::Lead, 0.0),
        ],
        macros: VOCODER_MACROS,
    },
    Factory {
        name: "Vocoder Chords",
        category: "Vocoder",
        values: &[
            SYNTH_CLEAN[0],
            SYNTH_CLEAN[1],
            SYNTH_CLEAN[2],
            (P::Vocoder, VOCODER),
            (P::Carrier, SUPERSAW),
            (P::Bands, BANDS_32),
            (P::CarrierFollow, 0.0),
            (P::CarrierRelease, 200.0),
            (P::Lead, 0.0),
        ],
        macros: VOCODER_MACROS,
    },
    Factory {
        name: "Voice + Vocoder",
        category: "Vocoder",
        values: &[
            SYNTH_CLEAN[0],
            SYNTH_CLEAN[1],
            SYNTH_CLEAN[2],
            (P::Vocoder, VOCODER),
            (P::Carrier, SQUARE),
            (P::CarrierOctave, OCT_DOWN_1),
            (P::Lead, 70.0),
            (P::VocoderLevel, 60.0),
        ],
        macros: VOCODER_MACROS,
    },
    Factory {
        name: "Whisper",
        category: "Vocoder",
        values: &[
            SYNTH_CLEAN[0],
            SYNTH_CLEAN[1],
            SYNTH_CLEAN[2],
            (P::Vocoder, VOCODER),
            (P::Carrier, NOISE),
            (P::Bands, BANDS_32),
            (P::Lead, 0.0),
            (P::Sibilance, 70.0),
        ],
        macros: VOCODER_MACROS,
    },
    Factory {
        name: "Talkbox Funk",
        category: "Vocoder",
        values: &[
            SYNTH_CLEAN[0],
            SYNTH_CLEAN[1],
            SYNTH_CLEAN[2],
            (P::Vocoder, TALKBOX),
            (P::Carrier, SQUARE),
            (P::CarrierGlide, 40.0),
            (P::Lead, 0.0),
        ],
        macros: VOCODER_MACROS,
    },
    Factory {
        name: "Robot Choir",
        category: "Vocoder",
        values: &[
            SYNTH_CLEAN[0],
            SYNTH_CLEAN[1],
            SYNTH_CLEAN[2],
            (P::Vocoder, VOCODER),
            (P::Carrier, SUPERSAW),
            (P::Bands, BANDS_16),
            (P::Detune, 30.0),
            (P::Lead, 0.0),
            (P::HarmonyOn, ON),
            (P::Harmony, H_MIDI),
            (P::HarmonyLevel, 40.0),
        ],
        macros: VOCODER_MACROS,
    },
    // Series chains (appended: program changes keep their numbers).
    Factory {
        name: "Vocoder in Thirds",
        category: "Vocoder",
        values: &[
            SYNTH_CLEAN[0],
            SYNTH_CLEAN[1],
            SYNTH_CLEAN[2],
            (P::Routing, SERIES),
            (P::Chain, CHAIN_CREATIVE_VOCODER_HARMONY),
            (P::Vocoder, VOCODER),
            (P::Carrier, SUPERSAW),
            (P::Detune, 20.0),
            (P::VocoderLevel, 100.0),
            (P::HarmonyOn, ON),
            (P::Scale, MAJOR),
            (P::Harmony, H_SCALE),
            (P::Harmony1, IV_3RD_UP),
            (P::Harmony2, IV_5TH_UP),
            (P::HarmonyLevel, 80.0),
        ],
        macros: VOCODER_MACROS,
    },
    Factory {
        name: "Glitched Choir Box",
        category: "Vocoder",
        values: &[
            SYNTH_CLEAN[0],
            SYNTH_CLEAN[1],
            SYNTH_CLEAN[2],
            (P::Routing, SERIES),
            (P::Chain, CHAIN_HARMONY_VOCODER_CREATIVE),
            (P::Vocoder, TALKBOX),
            (P::Carrier, SQUARE),
            (P::VocoderLevel, 70.0),
            (P::HarmonyOn, ON),
            (P::Scale, MAJOR),
            (P::Harmony, H_SCALE),
            (P::Harmony1, IV_3RD_UP),
            (P::HarmonyLevel, 70.0),
            (P::CreativeOn, ON),
            (P::Glitch, 30.0),
            (P::GlitchMode, 1.0),
        ],
        macros: VOCODER_MACROS,
    },
    // Throat and humanized harmonies (appended).
    Factory {
        name: "Child Voice",
        category: "Natural",
        values: vals!(
            Mode = NATURAL,
            Correction = 0.0,
            Throat = -75.0,
            ThroatPitch = 100.0
        ),
        macros: NATURAL_MACROS,
    },
    Factory {
        name: "Giant",
        category: "Natural",
        values: vals!(
            Mode = NATURAL,
            Correction = 0.0,
            Throat = 85.0,
            ThroatPitch = 60.0
        ),
        macros: NATURAL_MACROS,
    },
    Factory {
        name: "Human Choir",
        category: "Harmony",
        values: vals!(
            Mode = TUNE,
            HarmonyOn = ON,
            Scale = MAJOR,
            Harmony = H_SCALE,
            Harmony1 = IV_3RD_UP,
            Harmony2 = IV_5TH_UP,
            Harmony3 = IV_OCT_DOWN,
            Harmony4 = IV_3RD_DOWN,
            Spread = 90.0,
            HarmonyHumanize = 60.0,
            HarmonyVibrato = 60.0,
            HarmonyTiming = 60.0,
            Character = 50.0
        ),
        macros: HARMONY_MACROS,
    },
    // Voice effects (appended as well).
    Factory {
        name: "Live Vocal FX",
        category: "Natural",
        values: &[
            (P::FxOn, ON),
            (P::Gate, -60.0),
            (P::DeEss, 50.0),
            (P::CompThreshold, -24.0),
            (P::CompRatio, 3.0),
            (P::CompMakeup, 5.0),
            (P::DelayLevel, 25.0),
            (P::DelayFeedback, 30.0),
            (P::ReverbLevel, 50.0),
            (P::ReverbDecay, 1.8),
            (P::Duck, 50.0),
            (P::SendVoice, 25.0),
            (P::LowCut, 90.0),
            (P::Presence, 2.0),
            (P::Air, 2.5),
            (P::ReverbMod, 30.0),
        ],
        macros: FX_MACROS,
    },
    Factory {
        name: "Ambient Choir",
        category: "Harmony",
        values: &[
            SYNTH_CLEAN[0],
            SYNTH_CLEAN[1],
            SYNTH_CLEAN[2],
            (P::HarmonyOn, ON),
            (P::Scale, MAJOR),
            (P::Harmony, H_SCALE),
            (P::Harmony1, IV_3RD_UP),
            (P::Harmony2, IV_5TH_UP),
            (P::HarmonyLevel, 70.0),
            (P::FxOn, ON),
            (P::DeEss, 40.0),
            (P::CompThreshold, -20.0),
            (P::CompRatio, 2.5),
            (P::CompMakeup, 3.0),
            (P::DelayLevel, 30.0),
            (P::DelayPingPong, ON),
            (P::DelayFeedback, 45.0),
            (P::ReverbLevel, 80.0),
            (P::ReverbDecay, 4.5),
            (P::Duck, 40.0),
            (P::SendVoice, 10.0),
            (P::SendHarmony, 80.0),
            (P::ChorusOn, ON),
            (P::ChorusMix, 40.0),
            (P::DelayToReverb, 50.0),
            (P::ReverbSize, 80.0),
            (P::ReverbMod, 50.0),
        ],
        macros: FX_MACROS,
    },
];

/// Factory categories, in library order.
pub const CATEGORIES: [&str; 5] = ["Natural", "Tune", "Creative", "Harmony", "Vocoder"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_presets_are_valid_and_cover_every_category() {
        for c in CATEGORIES {
            assert!(
                FACTORY.iter().filter(|f| f.category == c).count() >= 3,
                "{c}"
            );
        }
        for (i, f) in FACTORY.iter().enumerate() {
            assert!(CATEGORIES.contains(&f.category), "{}", f.name);
            assert!(!FACTORY[..i].iter().any(|g| g.name == f.name), "{}", f.name);
            for &(id, v) in f.values {
                assert!(in_preset(id), "{}: {id:?}", f.name);
                assert_eq!(
                    id.spec().kind.sanitize(v),
                    v,
                    "{}: {id:?} out of range",
                    f.name
                );
            }
            for m in &f.macros {
                assert!(m.targets.len() <= crate::control::MACRO_SLOTS);
                for &(id, d) in m.targets {
                    assert!(crate::control::can_modulate(id) && d.abs() <= 1.0);
                }
            }
        }
        // Each preset turns on the layer of its category.
        for (cat, layer) in [
            ("Creative", P::CreativeOn),
            ("Harmony", P::HarmonyOn),
            ("Vocoder", P::VocoderOn),
        ] {
            for f in FACTORY.iter().filter(|f| f.category == cat) {
                assert_eq!(f.values().get(layer), 1.0, "{}", f.name);
            }
        }
    }

    #[test]
    fn text_round_trips_and_keeps_the_setup() {
        let mut p = FACTORY[7].to_preset();
        p.name = "My = Tune\nx".to_owned();
        let text = p.to_text();
        let q = Preset::parse(&text).expect("parse");
        assert_eq!(q.name, "My = Tune x");
        assert_eq!(q.values, p.values);
        assert_eq!(q.macros, p.macros);
        assert_eq!(q.macro_names, p.macro_names);
        assert!(!text.contains("bypass"));
        assert!(Preset::parse("mode = 1").is_none());

        let mut current = ParamValues::default();
        current.set(P::Voice, 3.0);
        current.set(P::Key, 6.0);
        let merged = merge(&current, &q.values);
        assert_eq!(merged.get(P::Voice), 3.0);
        assert_eq!(merged.get(P::Key), 6.0);
        assert_eq!(merged.get(P::Retune), 0.0);
        assert!(matches(&merged, &q.values));
        current.set(P::Retune, 30.0);
        assert!(!matches(&current, &q.values));
    }

    #[test]
    fn capture_reads_the_macros() {
        let c = ControlState::default();
        FACTORY[12].to_preset().recall_macros(&c);
        let p = Preset::capture("x", "User", &FACTORY[12].values(), &c);
        assert_eq!(p.macro_names[1], "Robot");
        assert_eq!(p.macros[1], FACTORY[12].macros[1].targets.to_vec());
    }
}
