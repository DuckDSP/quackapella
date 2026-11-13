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
//! A preset may also hold snapshots A and B (`snapshot_a = key:value,
//! ...`, as in the session state): they belong to the sound, so loading
//! a preset without them clears the ones stored. The factory styles
//! ([`STYLES`]) use them for a verse and a chorus, morphed by Macro 1.
//!
//! Missing parameters take their default, unknown lines are skipped, so
//! files stay readable across versions.

use std::fmt::Write as _;

use crate::control::{
    ControlState, MACRO_COUNT, SNAPSHOT_KEYS, by_key, lines, macro_key, parse_snapshot,
    parse_targets, snapshot_text, write_macros,
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
    /// One line for the browser: what it is for.
    pub note: &'static str,
    /// Snapshot B, as changes from the preset (A is the preset itself).
    /// Empty: the preset holds no snapshots.
    pub snapshot_b: &'static [(ParamId, f64)],
}

/// The fields a factory preset leaves at their default.
const PLAIN: Factory = Factory {
    name: "",
    category: "",
    values: &[],
    macros: NATURAL_MACROS,
    note: "",
    snapshot_b: &[],
};

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

    /// Snapshots A and B, when the preset holds them (allocation free).
    #[must_use]
    pub fn snapshots(&self) -> Option<[ParamValues; 2]> {
        if self.snapshot_b.is_empty() {
            return None;
        }
        let a = self.values();
        let mut b = a;
        for &(id, x) in self.snapshot_b {
            b.set(id, x);
        }
        Some([a, b])
    }

    #[must_use]
    pub fn to_preset(&self) -> Preset {
        Preset {
            name: self.name.to_owned(),
            category: self.category.to_owned(),
            values: self.values(),
            macro_names: self.macros.map(|m| m.name.to_owned()),
            macros: self.macros.map(|m| m.targets.to_vec()),
            snapshots: self
                .snapshots()
                .map_or([None, None], |[a, b]| [Some(a), Some(b)]),
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
    /// Snapshots A and B saved with the sound (see [`crate::snapshot`]).
    pub snapshots: [Option<ParamValues>; 2],
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
            snapshots: [control.snapshot(0), control.snapshot(1)],
        }
    }

    /// Whether the preset holds both snapshots (made to morph).
    #[must_use]
    pub fn has_snapshots(&self) -> bool {
        self.snapshots.iter().all(Option::is_some)
    }

    /// Recall the macros and the snapshots into `control` (editor
    /// thread). The snapshots belong to the sound: a preset without
    /// them clears the ones stored.
    pub fn recall_macros(&self, control: &ControlState) {
        for m in 0..MACRO_COUNT {
            control.set_targets(m, &self.macros[m]);
            control.set_name(m, &self.macro_names[m]);
        }
        for (s, snap) in self.snapshots.iter().enumerate() {
            match snap {
                Some(v) => control.store_snapshot(s, v),
                None => control.clear_snapshot(s),
            }
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
        for (snap, key) in self.snapshots.iter().zip(SNAPSHOT_KEYS) {
            if let Some(v) = snap {
                let _ = writeln!(s, "{key} = {}", snapshot_text(v));
            }
        }
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
            snapshots: [None, None],
        };
        let mut named = false;
        for (k, v) in lines(text) {
            match k {
                "name" => {
                    v.clone_into(&mut p.name);
                    named = true;
                }
                "category" => v.clone_into(&mut p.category),
                "snapshot_a" => p.snapshots[0] = Some(parse_snapshot(v)),
                "snapshot_b" => p.snapshots[1] = Some(parse_snapshot(v)),
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
    mac("Space", &[(P::VoiceReverb, 0.4), (P::ReverbLevel, 0.3)]),
    mac("Echo", &[(P::DelayLevel, 0.6), (P::DelayFeedback, 0.3)]),
    mac("Squash", &[(P::CompThreshold, -0.4), (P::CompMakeup, 0.2)]),
    mac("Duck", &[(P::Duck, 0.8), (P::DelayDuck, 0.8)]),
];

const SYNTH_CLEAN: [(ParamId, f64); 3] =
    [(P::Mode, TUNE), (P::VocoderOn, ON), (P::Correction, 60.0)];

// Style presets: more enum indices.
const MINOR: f64 = 2.0;
const DORIAN: f64 = 5.0;
const IV_5TH_DOWN: f64 = 17.0;
const PWM: f64 = 5.0;
const FOLLOW_FIFTH: f64 = 2.0;
const RATE_4TH: f64 = 0.0;
const DIV_16TH: f64 = 0.0;
const DIV_8T: f64 = 1.0;
const DIV_8: f64 = 2.0;
const DIV_4T: f64 = 3.0;
const DIV_8D: f64 = 4.0;
const DIV_4: f64 = 5.0;
const DIV_4D: f64 = 7.0;
const DIV_2: f64 = 8.0;
const TUBE: f64 = 0.0;
const TAPE: f64 = 1.0;
const HARD: f64 = 2.0;
const FUZZ: f64 = 3.0;
const LOFI: f64 = 4.0;

/// Moves between the preset's snapshots A and B (Morph On is switched
/// on when such a preset loads).
const VERSE_CHORUS: MacroDef = mac("Verse > Chorus", &[(P::Morph, 1.0)]);
const HARDNESS: MacroDef = TUNE_MACROS[0];
const SPACE: MacroDef = FX_MACROS[0];
const ECHO: MacroDef = FX_MACROS[1];
const GRIT: MacroDef = mac("Grit", &[(P::Drive, 0.5), (P::DriveTone, 0.15)]);
const WOBBLE: MacroDef = mac("Wobble", &[(P::ChorusDepth, 0.4), (P::DelayMod, 0.4)]);
const THROAT: MacroDef = mac("Throat", &[(P::Throat, 0.25)]);
const STACK: MacroDef = mac(
    "Stack",
    &[(P::HarmonyLevel, 0.3), (P::Spread, 0.2), (P::Double, 0.2)],
);

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
        ..PLAIN
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
        ..PLAIN
    },
    Factory {
        name: "Octave Down",
        category: "Natural",
        values: vals!(Mode = NATURAL, Correction = 0.0, Pitch = -12.0),
        macros: NATURAL_MACROS,
        ..PLAIN
    },
    Factory {
        name: "Fifth Up",
        category: "Natural",
        values: vals!(Mode = NATURAL, Correction = 0.0, Pitch = 7.0),
        macros: NATURAL_MACROS,
        ..PLAIN
    },
    Factory {
        name: "Darker Voice",
        category: "Natural",
        values: vals!(Mode = NATURAL, Correction = 0.0, Formant = -2.5),
        macros: NATURAL_MACROS,
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
        ..PLAIN
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
            (P::DelayDuck, 50.0),
            (P::VoiceReverb, 25.0),
            (P::VoiceDelay, 25.0),
            (P::LowCut, 90.0),
            (P::Presence, 2.0),
            (P::Air, 2.5),
            (P::ReverbMod, 30.0),
        ],
        macros: FX_MACROS,
        ..PLAIN
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
            (P::DelayDuck, 40.0),
            (P::VoiceReverb, 10.0),
            (P::HarmonyReverb, 80.0),
            (P::ChorusOn, ON),
            (P::VoiceChorus, 20.0),
            (P::HarmonyChorus, 70.0),
            (P::HarmonyDelay, 70.0),
            (P::VoiceDelay, 10.0),
            (P::DelayToReverb, 50.0),
            (P::ReverbSize, 80.0),
            (P::ReverbMod, 50.0),
        ],
        macros: FX_MACROS,
        ..PLAIN
    },
    // ---- Styles (appended: program changes keep their numbers) ----
    // Finished sounds for a genre, each using the effects and, where
    // it says "B", snapshot B for the chorus or the drop (Macro 1 or the
    // A/B buttons move between them).
    Factory {
        name: "Radio Pop",
        category: "Pop",
        note: "Polished pop lead: quick tune, bright EQ, slap delay. B adds a double and a third.",
        values: vals!(
            Mode = TUNE,
            Scale = MAJOR,
            Retune = 20.0,
            Humanize = 25.0,
            Vibrato = 70.0,
            Harmony1 = IV_3RD_UP,
            Harmony2 = IV_OFF,
            HarmonyLevel = 0.0,
            FxOn = ON,
            Gate = -60.0,
            DeEss = 45.0,
            CompThreshold = -22.0,
            CompRatio = 3.0,
            CompAttack = 8.0,
            CompMakeup = 4.0,
            LowCut = 100.0,
            Presence = 2.0,
            Air = 3.0,
            DelayDivision = DIV_8D,
            DelayLevel = 20.0,
            DelayFeedback = 25.0,
            DelayDuck = 70.0,
            ReverbLevel = 45.0,
            ReverbDecay = 1.6,
            ReverbSize = 45.0,
            Duck = 40.0,
            VoiceReverb = 20.0,
            VoiceDelay = 20.0,
            OutDeEss = 25.0,
            OutComp = 30.0,
            OutCeiling = -1.0
        ),
        snapshot_b: vals!(
            HarmonyOn = ON,
            HarmonyLevel = 55.0,
            Double = 45.0,
            HarmonyReverb = 45.0,
            HarmonyDelay = 30.0,
            VoiceReverb = 28.0,
            ReverbLevel = 55.0
        ),
        macros: [VERSE_CHORUS, HARDNESS, SPACE, ECHO],
    },
    Factory {
        name: "Pop Stack",
        category: "Pop",
        note: "Lead with stacked doubles and a third above, panned wide, chorus on the stack.",
        values: vals!(
            Mode = TUNE,
            Scale = MAJOR,
            Retune = 30.0,
            Humanize = 20.0,
            Vibrato = 70.0,
            HarmonyOn = ON,
            Harmony1 = IV_3RD_UP,
            Harmony2 = IV_OCT_DOWN,
            HarmonyLevel = 60.0,
            HarmonyLevel2 = 50.0,
            Double = 60.0,
            Spread = 80.0,
            HarmonyHumanize = 50.0,
            HarmonyTiming = 40.0,
            HarmonyVibrato = 30.0,
            Character = 35.0,
            FxOn = ON,
            DeEss = 40.0,
            CompThreshold = -20.0,
            CompRatio = 3.0,
            CompMakeup = 3.0,
            LowCut = 90.0,
            Air = 2.0,
            ChorusOn = ON,
            VoiceChorus = 0.0,
            HarmonyChorus = 50.0,
            ChorusRate = 0.6,
            ChorusDepth = 35.0,
            DelayDivision = DIV_4,
            DelayLevel = 15.0,
            DelayDuck = 60.0,
            ReverbLevel = 50.0,
            VoiceReverb = 18.0,
            HarmonyReverb = 50.0,
            OutComp = 25.0
        ),
        macros: [STACK, mac("Double", &[(P::Double, 0.4)]), SPACE, DRY_WET],
        ..PLAIN
    },
    Factory {
        name: "Pop Ballad",
        category: "Pop",
        note: "Discreet correction for slow songs, long reverb, a quarter-note delay that waits for the gaps.",
        values: vals!(
            Mode = NATURAL,
            Correction = 70.0,
            Retune = 110.0,
            Humanize = 45.0,
            Vibrato = 100.0,
            FxOn = ON,
            DeEss = 35.0,
            CompThreshold = -24.0,
            CompRatio = 2.5,
            CompAttack = 15.0,
            CompRelease = 200.0,
            CompMakeup = 3.0,
            LowCut = 80.0,
            Air = 2.5,
            DelayDivision = DIV_4,
            DelayLevel = 25.0,
            DelayFeedback = 35.0,
            DelayDuck = 80.0,
            DelayToReverb = 40.0,
            ReverbLevel = 60.0,
            ReverbDecay = 3.2,
            ReverbSize = 70.0,
            ReverbPredelay = 40.0,
            ReverbDamping = 55.0,
            Duck = 50.0,
            VoiceReverb = 30.0,
            VoiceDelay = 25.0,
            OutComp = 20.0
        ),
        macros: [NATURAL_MACROS[0], SPACE, ECHO, DRY_WET],
        ..PLAIN
    },
    Factory {
        name: "Silky R&B",
        category: "R&B",
        note: "Smooth minor lead with slow glides, a low third underneath, dark reverb. B adds a high third and doubles.",
        values: vals!(
            Mode = TUNE,
            Scale = MINOR,
            Retune = 45.0,
            Transition = 45.0,
            Humanize = 30.0,
            Vibrato = 85.0,
            Throat = 15.0,
            HarmonyOn = ON,
            Harmony1 = IV_3RD_DOWN,
            Harmony2 = IV_3RD_UP,
            HarmonyLevel2 = 0.0,
            HarmonyLevel = 40.0,
            Glide = 60.0,
            HarmonyHumanize = 50.0,
            FxOn = ON,
            DeEss = 45.0,
            CompThreshold = -22.0,
            CompRatio = 3.0,
            CompMakeup = 4.0,
            LowCut = 90.0,
            Presence = 1.5,
            Air = 3.0,
            DelayDivision = DIV_4T,
            DelayLevel = 18.0,
            DelayFeedback = 30.0,
            DelayDuck = 60.0,
            DelayMod = 15.0,
            ReverbLevel = 55.0,
            ReverbDecay = 2.4,
            ReverbDamping = 70.0,
            ReverbSize = 60.0,
            VoiceReverb = 22.0,
            HarmonyReverb = 40.0,
            ChorusOn = ON,
            VoiceChorus = 0.0,
            HarmonyChorus = 40.0,
            OutDeEss = 30.0,
            OutComp = 25.0
        ),
        snapshot_b: vals!(HarmonyLevel2 = 100.0, HarmonyLevel = 55.0, Double = 35.0),
        macros: [
            VERSE_CHORUS,
            mac("Glide", &[(P::Transition, 0.3)]),
            mac("Harmonies", &[(P::HarmonyLevel, 0.4)]),
            SPACE,
        ],
    },
    Factory {
        name: "Neo-Soul Stack",
        category: "R&B",
        note: "Dorian three-part stack with loose timing and warm tube drive, like an overdubbed section.",
        values: vals!(
            Mode = TUNE,
            Scale = DORIAN,
            Retune = 60.0,
            Humanize = 40.0,
            Vibrato = 90.0,
            HarmonyOn = ON,
            Harmony1 = IV_3RD_UP,
            Harmony2 = IV_5TH_UP,
            Harmony3 = IV_3RD_DOWN,
            HarmonyLevel = 55.0,
            Spread = 70.0,
            HarmonyHumanize = 70.0,
            HarmonyTiming = 65.0,
            HarmonyVibrato = 50.0,
            Character = 45.0,
            Expression = 80.0,
            FxOn = ON,
            CompThreshold = -20.0,
            CompRatio = 2.5,
            CompMakeup = 3.0,
            DeEss = 35.0,
            LowCut = 80.0,
            Presence = -1.0,
            Air = 1.0,
            DriveOn = ON,
            DriveStyle = TUBE,
            Drive = 30.0,
            DriveTone = 45.0,
            VoiceDrive = 60.0,
            HarmonyDrive = 40.0,
            ReverbLevel = 45.0,
            ReverbDecay = 1.4,
            ReverbSize = 35.0,
            ReverbDamping = 65.0,
            VoiceReverb = 15.0,
            HarmonyReverb = 35.0,
            ChorusOn = ON,
            VoiceChorus = 0.0,
            HarmonyChorus = 30.0,
            OutComp = 20.0
        ),
        macros: [
            HARMONY_MACROS[0],
            GRIT,
            mac(
                "Looseness",
                &[(P::HarmonyTiming, 0.3), (P::HarmonyHumanize, 0.3)],
            ),
            SPACE,
        ],
        ..PLAIN
    },
    Factory {
        name: "Trap Lead",
        category: "Trap",
        note: "Hard-tuned minor lead: instant snap, crisp top, ping-pong quarters. B drops an octave and a sub under the hook.",
        values: vals!(
            Mode = TUNE,
            Scale = MINOR,
            Retune = 0.0,
            Humanize = 0.0,
            Vibrato = 0.0,
            Transition = 12.0,
            OctaveDown = 0.0,
            CreativeDrive = 40.0,
            FxOn = ON,
            Gate = -55.0,
            DeEss = 55.0,
            CompThreshold = -26.0,
            CompRatio = 4.0,
            CompAttack = 3.0,
            CompRelease = 90.0,
            CompMakeup = 6.0,
            LowCut = 120.0,
            Presence = 3.0,
            Air = 4.0,
            DriveOn = ON,
            DriveStyle = TAPE,
            Drive = 25.0,
            VoiceDrive = 60.0,
            DelayDivision = DIV_4,
            DelayPingPong = ON,
            DelayLevel = 22.0,
            DelayFeedback = 30.0,
            DelayDuck = 75.0,
            DelayWidth = 60.0,
            ReverbLevel = 35.0,
            ReverbDecay = 1.2,
            ReverbSize = 30.0,
            VoiceReverb = 12.0,
            Duck = 50.0,
            OutDeEss = 35.0,
            OutComp = 45.0,
            OutCeiling = -1.0
        ),
        snapshot_b: vals!(CreativeOn = ON, OctaveDown = 45.0, Sub = 20.0),
        macros: [
            VERSE_CHORUS,
            mac(
                "Soften",
                &[(P::Retune, 0.1), (P::Humanize, 0.3), (P::Transition, 0.2)],
            ),
            ECHO,
            GRIT,
        ],
    },
    Factory {
        name: "Dark Trap",
        category: "Trap",
        note: "A bigger, darker throat with an octave and a sub below, hard drive on the low layer.",
        values: vals!(
            Mode = TUNE,
            Scale = MINOR,
            Retune = 0.0,
            Humanize = 5.0,
            Vibrato = 0.0,
            Throat = 45.0,
            Formant = -1.5,
            CreativeOn = ON,
            OctaveDown = 40.0,
            Sub = 25.0,
            FxOn = ON,
            DeEss = 45.0,
            CompThreshold = -24.0,
            CompRatio = 4.0,
            CompMakeup = 5.0,
            LowCut = 60.0,
            Presence = 2.0,
            Air = 1.0,
            DriveOn = ON,
            DriveStyle = HARD,
            Drive = 35.0,
            DriveTone = 40.0,
            VoiceDrive = 25.0,
            CreativeDrive = 80.0,
            DelayDivision = DIV_8D,
            DelayLevel = 15.0,
            DelayFeedback = 40.0,
            ReverbLevel = 50.0,
            ReverbDecay = 2.8,
            ReverbDamping = 75.0,
            ReverbSize = 60.0,
            VoiceReverb = 15.0,
            CreativeReverb = 25.0,
            OutComp = 40.0,
            OutCeiling = -1.0
        ),
        macros: [THROAT, CREATIVE_MACROS[3], GRIT, SPACE],
        ..PLAIN
    },
    Factory {
        name: "Trap Adlibs",
        category: "Trap",
        note: "Adlib channel: hard tune, octave up, a fifth above, 1/16 stutters thrown into a wide delay. B glitches harder.",
        values: vals!(
            Mode = TUNE,
            Scale = MINOR,
            Retune = 0.0,
            Humanize = 0.0,
            Vibrato = 0.0,
            Lead = 70.0,
            CreativeOn = ON,
            OctaveUp = 35.0,
            Glitch = 20.0,
            GlitchMode = GLITCH_STUTTER,
            GlitchRate = RATE_16TH,
            HarmonyOn = ON,
            Harmony1 = IV_5TH_UP,
            Harmony2 = IV_OFF,
            HarmonyLevel = 45.0,
            Spread = 90.0,
            FxOn = ON,
            DeEss = 50.0,
            CompThreshold = -24.0,
            CompRatio = 4.0,
            CompMakeup = 4.0,
            LowCut = 200.0,
            Presence = 2.0,
            Air = 5.0,
            DelayDivision = DIV_8,
            DelayPingPong = ON,
            DelayLevel = 40.0,
            DelayFeedback = 45.0,
            DelayWidth = 100.0,
            DelayDuck = 50.0,
            VoiceDelay = 35.0,
            CreativeDelay = 60.0,
            HarmonyDelay = 50.0,
            ReverbLevel = 55.0,
            ReverbDecay = 2.2,
            VoiceReverb = 25.0,
            CreativeReverb = 45.0,
            HarmonyReverb = 45.0,
            ChorusOn = ON,
            VoiceChorus = 0.0,
            CreativeChorus = 40.0,
            HarmonyChorus = 40.0,
            OutComp = 35.0,
            OutCeiling = -1.0
        ),
        snapshot_b: vals!(Glitch = 60.0, DelayFeedback = 65.0),
        macros: [
            VERSE_CHORUS,
            CREATIVE_MACROS[2],
            ECHO,
            mac("Octave Up", &[(P::OctaveUp, 0.4)]),
        ],
    },
    Factory {
        name: "French Touch Talkbox",
        category: "Electro",
        note: "Talkbox on a PWM synth with a sub, singing your tuned note; funky glide, tube grit. B widens it.",
        values: vals!(
            Mode = TUNE,
            Scale = MAJOR,
            Retune = 0.0,
            Humanize = 0.0,
            Vibrato = 0.0,
            VocoderOn = ON,
            Vocoder = TALKBOX,
            Carrier = PWM,
            CarrierSub = 35.0,
            CarrierGlide = 35.0,
            Consonants = 50.0,
            Lead = 0.0,
            VocoderLevel = 90.0,
            FxOn = ON,
            CompThreshold = -20.0,
            CompRatio = 3.0,
            CompMakeup = 3.0,
            DriveOn = ON,
            DriveStyle = TUBE,
            Drive = 35.0,
            VocoderDrive = 70.0,
            ChorusOn = ON,
            VoiceChorus = 0.0,
            VocoderChorus = 35.0,
            ChorusRate = 0.5,
            DelayDivision = DIV_8D,
            DelayLevel = 18.0,
            DelayFeedback = 30.0,
            VocoderDelay = 30.0,
            ReverbLevel = 40.0,
            ReverbDecay = 1.3,
            VocoderReverb = 20.0,
            OutComp = 30.0
        ),
        snapshot_b: vals!(Ensemble = 55.0, VocoderWidth = 90.0, VocoderReverb = 40.0),
        macros: [
            VERSE_CHORUS,
            VOCODER_MACROS[0],
            mac("Glide", &[(P::CarrierGlide, 0.3)]),
            GRIT,
        ],
    },
    Factory {
        name: "Electro Vocoder",
        category: "Electro",
        note: "Wide 32-band supersaw vocoder playing a fifth over your note, crisp consonants, ping-pong eighths.",
        values: vals!(
            OutputGain = 2.0,
            Mode = TUNE,
            Correction = 60.0,
            VocoderOn = ON,
            Vocoder = VOCODER,
            Carrier = SUPERSAW,
            Bands = BANDS_32,
            FollowChord = FOLLOW_FIFTH,
            Detune = 18.0,
            VocoderResonance = 60.0,
            Consonants = 60.0,
            VocoderTilt = 3.0,
            VocoderGate = -55.0,
            Ensemble = 40.0,
            VocoderWidth = 85.0,
            Lead = 0.0,
            FxOn = ON,
            DelayDivision = DIV_8,
            DelayPingPong = ON,
            DelayLevel = 25.0,
            DelayFeedback = 40.0,
            VocoderDelay = 45.0,
            ReverbLevel = 45.0,
            VocoderReverb = 35.0,
            ChorusOn = ON,
            VoiceChorus = 0.0,
            VocoderChorus = 30.0,
            OutComp = 30.0,
            OutCeiling = -1.0
        ),
        macros: [
            VOCODER_MACROS[0],
            VOCODER_MACROS[1],
            VOCODER_MACROS[2],
            ECHO,
        ],
        ..PLAIN
    },
    Factory {
        name: "Synthwave Duet",
        category: "Electro",
        note: "Your voice with a saw vocoder an octave below, lush chorus and long reverb: eighties synth-pop.",
        values: vals!(
            OutputGain = 2.0,
            Mode = TUNE,
            Scale = MAJOR,
            Retune = 25.0,
            Humanize = 20.0,
            Vibrato = 60.0,
            VocoderOn = ON,
            Vocoder = VOCODER,
            Carrier = SAW,
            CarrierOctave = OCT_DOWN_1,
            Detune = 20.0,
            Ensemble = 60.0,
            Lead = 75.0,
            VocoderLevel = 60.0,
            FxOn = ON,
            DeEss = 35.0,
            CompThreshold = -22.0,
            CompRatio = 3.0,
            CompMakeup = 3.0,
            LowCut = 100.0,
            Air = 2.0,
            ChorusOn = ON,
            VoiceChorus = 35.0,
            VocoderChorus = 70.0,
            ChorusRate = 0.4,
            ChorusDepth = 55.0,
            DelayDivision = DIV_4D,
            DelayLevel = 25.0,
            DelayFeedback = 40.0,
            DelayToReverb = 50.0,
            ReverbLevel = 65.0,
            ReverbDecay = 3.5,
            ReverbSize = 80.0,
            ReverbMod = 50.0,
            VoiceReverb = 25.0,
            VocoderReverb = 45.0,
            DriveOn = ON,
            DriveStyle = TAPE,
            Drive = 20.0,
            VoiceDrive = 40.0,
            VocoderDrive = 60.0,
            OutComp = 25.0
        ),
        macros: [VOCODER_MACROS[3], WOBBLE, SPACE, ECHO],
        ..PLAIN
    },
    Factory {
        name: "Gospel Choir",
        category: "Choir",
        note: "Four loose voices around the lead in the scale, wide and warm. B: the full choir takes over.",
        values: vals!(
            Mode = TUNE,
            Scale = MAJOR,
            Retune = 30.0,
            Humanize = 30.0,
            HarmonyOn = ON,
            Harmony1 = IV_3RD_UP,
            Harmony2 = IV_5TH_UP,
            Harmony3 = IV_OCT_DOWN,
            Harmony4 = IV_3RD_DOWN,
            HarmonyLevel = 70.0,
            Spread = 85.0,
            HarmonyHumanize = 65.0,
            HarmonyVibrato = 55.0,
            HarmonyTiming = 55.0,
            Character = 45.0,
            Double = 30.0,
            FxOn = ON,
            DeEss = 40.0,
            CompThreshold = -22.0,
            CompRatio = 2.5,
            CompMakeup = 3.0,
            LowCut = 80.0,
            Air = 2.0,
            ReverbLevel = 60.0,
            ReverbDecay = 2.8,
            ReverbSize = 70.0,
            ReverbPredelay = 30.0,
            VoiceReverb = 20.0,
            HarmonyReverb = 55.0,
            ChorusOn = ON,
            VoiceChorus = 0.0,
            HarmonyChorus = 25.0,
            OutComp = 25.0
        ),
        snapshot_b: vals!(
            Lead = 80.0,
            HarmonyLevel = 95.0,
            Double = 60.0,
            HarmonyReverb = 70.0
        ),
        macros: [VERSE_CHORUS, HARMONY_MACROS[0], HARMONY_MACROS[3], SPACE],
    },
    Factory {
        name: "Angel Choir",
        category: "Choir",
        note: "Airy voices an octave and a fifth above, chorused, in a huge slow reverb.",
        values: vals!(
            Mode = TUNE,
            Scale = MAJOR,
            Retune = 40.0,
            HarmonyOn = ON,
            Harmony1 = IV_OCT_UP,
            Harmony2 = IV_5TH_UP,
            HarmonyLevel = 55.0,
            Spread = 100.0,
            HarmonyHumanize = 55.0,
            HarmonyVibrato = 40.0,
            Character = 20.0,
            FxOn = ON,
            DeEss = 50.0,
            LowCut = 120.0,
            Presence = -1.0,
            Air = 4.0,
            ChorusOn = ON,
            VoiceChorus = 10.0,
            HarmonyChorus = 60.0,
            ChorusRate = 0.3,
            ChorusDepth = 55.0,
            DelayDivision = DIV_4,
            DelayLevel = 25.0,
            DelayFeedback = 50.0,
            DelayToReverb = 70.0,
            VoiceDelay = 15.0,
            HarmonyDelay = 50.0,
            ReverbLevel = 80.0,
            ReverbDecay = 7.0,
            ReverbSize = 95.0,
            ReverbMod = 60.0,
            ReverbPredelay = 60.0,
            ReverbDamping = 35.0,
            VoiceReverb = 30.0,
            HarmonyReverb = 80.0,
            Duck = 35.0,
            OutComp = 20.0
        ),
        macros: [
            HARMONY_MACROS[0],
            SPACE,
            mac("Air", &[(P::Air, 0.2), (P::DeEss, 0.2)]),
            DRY_WET,
        ],
        ..PLAIN
    },
    Factory {
        name: "Barbershop",
        category: "Choir",
        note: "Close three-part harmony under the lead, tight timing, small room.",
        values: vals!(
            Mode = TUNE,
            Scale = MAJOR,
            Retune = 20.0,
            Humanize = 20.0,
            HarmonyOn = ON,
            Harmony1 = IV_3RD_DOWN,
            Harmony2 = IV_5TH_DOWN,
            Harmony3 = IV_OCT_DOWN,
            HarmonyLevel = 70.0,
            Spread = 35.0,
            HarmonyHumanize = 35.0,
            HarmonyTiming = 20.0,
            HarmonyVibrato = 45.0,
            Character = 30.0,
            HarmonyPan1 = -40.0,
            HarmonyPan2 = 40.0,
            HarmonyPan3 = 0.0,
            FxOn = ON,
            CompThreshold = -22.0,
            CompRatio = 2.5,
            CompMakeup = 3.0,
            DeEss = 35.0,
            LowCut = 70.0,
            Presence = 1.0,
            ReverbLevel = 40.0,
            ReverbDecay = 0.9,
            ReverbSize = 25.0,
            ReverbDamping = 60.0,
            VoiceReverb = 15.0,
            HarmonyReverb = 30.0,
            OutComp = 20.0
        ),
        macros: [HARMONY_MACROS[0], HARMONY_MACROS[3], SPACE, DRY_WET],
        ..PLAIN
    },
    Factory {
        name: "Android",
        category: "Robot",
        note: "Hard-tuned robot layer alone, hard drive, short metallic room. B: darker, an octave below.",
        values: vals!(
            Mode = TUNE,
            Retune = 0.0,
            Humanize = 0.0,
            Vibrato = 0.0,
            CreativeOn = ON,
            Lead = 0.0,
            Robot = 100.0,
            Formant = -1.0,
            OctaveDown = 0.0,
            FxOn = ON,
            Gate = -50.0,
            CompThreshold = -20.0,
            CompRatio = 4.0,
            CompMakeup = 4.0,
            LowCut = 120.0,
            Presence = 3.0,
            DriveOn = ON,
            DriveStyle = HARD,
            Drive = 30.0,
            CreativeDrive = 70.0,
            DelayDivision = DIV_16TH,
            DelayLevel = 15.0,
            DelayFeedback = 20.0,
            CreativeDelay = 30.0,
            ReverbLevel = 35.0,
            ReverbDecay = 0.6,
            ReverbDamping = 15.0,
            ReverbSize = 20.0,
            CreativeReverb = 25.0,
            OutComp = 30.0
        ),
        snapshot_b: vals!(OctaveDown = 50.0, Formant = -4.0),
        macros: [
            VERSE_CHORUS,
            mac("Human", &[(P::Robot, -0.7), (P::Lead, 0.5)]),
            GRIT,
            SPACE,
        ],
    },
    Factory {
        name: "Broken Machine",
        category: "Robot",
        note: "A robot that stutters, through fuzz and a noisy vocoder whisper. B reverses and breaks more.",
        values: vals!(
            OutputGain = -4.0,
            Mode = TUNE,
            Retune = 0.0,
            Humanize = 0.0,
            Vibrato = 0.0,
            CreativeOn = ON,
            Lead = 0.0,
            Robot = 70.0,
            Glitch = 35.0,
            GlitchMode = GLITCH_STUTTER,
            GlitchRate = RATE_16TH,
            VocoderOn = ON,
            Vocoder = VOCODER,
            Carrier = NOISE,
            Bands = BANDS_16,
            VocoderLevel = 30.0,
            Sibilance = 60.0,
            FxOn = ON,
            Gate = -50.0,
            DriveOn = ON,
            DriveStyle = FUZZ,
            Drive = 45.0,
            DriveTone = 50.0,
            CreativeDrive = 80.0,
            VocoderDrive = 40.0,
            LowCut = 150.0,
            Presence = 2.0,
            DelayDivision = DIV_8T,
            DelayLevel = 20.0,
            DelayFeedback = 35.0,
            CreativeDelay = 35.0,
            ReverbLevel = 30.0,
            ReverbDecay = 1.0,
            OutComp = 40.0,
            OutCeiling = -2.0
        ),
        snapshot_b: vals!(
            Glitch = 60.0,
            GlitchMode = GLITCH_REVERSE,
            VocoderLevel = 60.0
        ),
        macros: [
            VERSE_CHORUS,
            CREATIVE_MACROS[2],
            GRIT,
            mac("Noise", &[(P::VocoderLevel, 0.5)]),
        ],
    },
    Factory {
        name: "Cyborg Choir",
        category: "Robot",
        note: "In series: a robot lead and its scale harmonies into a square-wave vocoder.",
        values: vals!(
            OutputGain = -5.0,
            Mode = TUNE,
            Scale = MAJOR,
            Retune = 0.0,
            Humanize = 0.0,
            Vibrato = 0.0,
            Routing = SERIES,
            Chain = CHAIN_HARMONY_VOCODER_CREATIVE,
            CreativeOn = ON,
            Robot = 60.0,
            HarmonyOn = ON,
            Harmony1 = IV_3RD_UP,
            Harmony2 = IV_5TH_UP,
            Harmony3 = IV_OCT_DOWN,
            HarmonyLevel = 70.0,
            Spread = 90.0,
            VocoderOn = ON,
            Vocoder = VOCODER,
            Carrier = SQUARE,
            VocoderLevel = 55.0,
            FxOn = ON,
            CompThreshold = -20.0,
            CompRatio = 3.0,
            CompMakeup = 3.0,
            DriveOn = ON,
            DriveStyle = TUBE,
            Drive = 30.0,
            ChorusOn = ON,
            VoiceChorus = 30.0,
            DelayDivision = DIV_8D,
            DelayLevel = 20.0,
            VoiceDelay = 25.0,
            ReverbLevel = 55.0,
            ReverbDecay = 2.5,
            VoiceReverb = 30.0,
            OutComp = 30.0
        ),
        macros: [
            HARMONY_MACROS[0],
            mac("Robot", &[(P::Robot, 0.4)]),
            mac("Vocoder", &[(P::VocoderLevel, 0.45)]),
            SPACE,
        ],
        ..PLAIN
    },
    Factory {
        name: "Giant's Lair",
        category: "Cinematic",
        note: "A giant's throat with octaves below, in a vast dark hall. B: the beast wakes up.",
        values: vals!(
            OutputGain = -3.0,
            Mode = TUNE,
            Correction = 0.0,
            Throat = 80.0,
            ThroatPitch = 40.0,
            CreativeOn = ON,
            OctaveDown = 35.0,
            Sub = 20.0,
            FxOn = ON,
            CompThreshold = -24.0,
            CompRatio = 3.0,
            CompMakeup = 4.0,
            LowCut = 40.0,
            Air = -3.0,
            DriveOn = ON,
            DriveStyle = TUBE,
            Drive = 20.0,
            VoiceDrive = 30.0,
            CreativeDrive = 60.0,
            DelayDivision = DIV_2,
            DelayLevel = 18.0,
            DelayFeedback = 45.0,
            DelayToReverb = 50.0,
            ReverbLevel = 70.0,
            ReverbDecay = 7.5,
            ReverbSize = 95.0,
            ReverbPredelay = 80.0,
            ReverbDamping = 75.0,
            ReverbLowCut = 60.0,
            VoiceReverb = 35.0,
            CreativeReverb = 45.0,
            OutComp = 30.0,
            OutCeiling = -1.0
        ),
        snapshot_b: vals!(
            Throat = 100.0,
            OctaveDown = 70.0,
            Sub = 45.0,
            ReverbLevel = 85.0
        ),
        macros: [
            VERSE_CHORUS,
            mac("Size", &[(P::Throat, -0.4)]),
            CREATIVE_MACROS[3],
            SPACE,
        ],
    },
    Factory {
        name: "Ethereal Pad",
        category: "Cinematic",
        note: "Your voice blooming into a slow supersaw pad, a fifth and an octave above, endless reverb. B: all pad.",
        values: vals!(
            Mode = TUNE,
            Scale = MAJOR,
            Correction = 70.0,
            Retune = 60.0,
            HarmonyOn = ON,
            Harmony1 = IV_5TH_UP,
            Harmony2 = IV_OCT_UP,
            HarmonyLevel = 45.0,
            Spread = 100.0,
            HarmonyHumanize = 50.0,
            VocoderOn = ON,
            Vocoder = VOCODER,
            Carrier = SUPERSAW,
            Bands = BANDS_32,
            CarrierRelease = 400.0,
            Response = 40.0,
            Ensemble = 80.0,
            Detune = 25.0,
            VocoderWidth = 100.0,
            VocoderLevel = 45.0,
            Lead = 70.0,
            FxOn = ON,
            DeEss = 40.0,
            LowCut = 100.0,
            Air = 3.0,
            ChorusOn = ON,
            VoiceChorus = 20.0,
            HarmonyChorus = 60.0,
            VocoderChorus = 60.0,
            ChorusRate = 0.25,
            ChorusDepth = 60.0,
            DelayDivision = DIV_4D,
            DelayLevel = 30.0,
            DelayFeedback = 55.0,
            DelayMod = 25.0,
            DelayToReverb = 75.0,
            VoiceDelay = 20.0,
            HarmonyDelay = 50.0,
            VocoderDelay = 50.0,
            ReverbLevel = 85.0,
            ReverbDecay = 9.0,
            ReverbSize = 100.0,
            ReverbMod = 70.0,
            ReverbPredelay = 50.0,
            ReverbDamping = 40.0,
            VoiceReverb = 35.0,
            HarmonyReverb = 75.0,
            VocoderReverb = 80.0,
            Duck = 30.0,
            OutComp = 15.0
        ),
        snapshot_b: vals!(Lead = 40.0, VocoderLevel = 80.0, HarmonyLevel = 70.0),
        macros: [
            VERSE_CHORUS,
            SPACE,
            mac("Release", &[(P::CarrierRelease, 0.4)]),
            DRY_WET,
        ],
    },
    Factory {
        name: "Lo-Fi Tape",
        category: "Lo-Fi",
        note: "Gentle correction through a worn tape: band-limited, wobbly chorus and delay, crunchy.",
        values: vals!(
            OutputGain = 2.0,
            Mode = NATURAL,
            Correction = 60.0,
            Retune = 100.0,
            FxOn = ON,
            LowCut = 220.0,
            Presence = 3.0,
            Air = -6.0,
            CompThreshold = -26.0,
            CompRatio = 4.0,
            CompMakeup = 5.0,
            DriveOn = ON,
            DriveStyle = LOFI,
            Drive = 50.0,
            DriveTone = 35.0,
            VoiceDrive = 70.0,
            ChorusOn = ON,
            VoiceChorus = 35.0,
            ChorusRate = 0.35,
            ChorusDepth = 60.0,
            DelayDivision = DIV_4,
            DelayLevel = 22.0,
            DelayFeedback = 40.0,
            DelayMod = 55.0,
            DelayLowCut = 300.0,
            ReverbLevel = 40.0,
            ReverbDecay = 1.2,
            ReverbDamping = 80.0,
            ReverbSize = 40.0,
            VoiceReverb = 20.0,
            OutComp = 35.0
        ),
        macros: [WOBBLE, GRIT, ECHO, DRY_WET],
        ..PLAIN
    },
    Factory {
        name: "Old Radio",
        category: "Lo-Fi",
        note: "A small, squashed, band-limited voice breaking up like an old speaker.",
        values: vals!(
            OutputGain = -4.0,
            Mode = NATURAL,
            Correction = 0.0,
            FxOn = ON,
            LowCut = 400.0,
            Presence = 9.0,
            Air = -12.0,
            CompThreshold = -30.0,
            CompRatio = 6.0,
            CompMakeup = 8.0,
            DriveOn = ON,
            DriveStyle = FUZZ,
            Drive = 30.0,
            DriveTone = 30.0,
            VoiceDrive = 80.0,
            ReverbLevel = 25.0,
            ReverbDecay = 0.5,
            ReverbSize = 10.0,
            ReverbDamping = 80.0,
            ReverbWidth = 0.0,
            VoiceReverb = 10.0,
            OutComp = 50.0,
            OutCeiling = -3.0
        ),
        macros: [
            GRIT,
            mac("Band", &[(P::LowCut, -0.3), (P::Presence, -0.2)]),
            SPACE,
            DRY_WET,
        ],
        ..PLAIN
    },
    Factory {
        name: "Cassette Stops",
        category: "Lo-Fi",
        note: "A tape copy of your voice that slows to a stop on the beat, warm and wobbly. B stops more often.",
        values: vals!(
            Mode = TUNE,
            Correction = 60.0,
            Retune = 60.0,
            CreativeOn = ON,
            Lead = 0.0,
            Glitch = 20.0,
            GlitchMode = GLITCH_TAPE,
            GlitchRate = RATE_4TH,
            OctaveDown = 15.0,
            FxOn = ON,
            LowCut = 180.0,
            Air = -5.0,
            DriveOn = ON,
            DriveStyle = TAPE,
            Drive = 45.0,
            CreativeDrive = 70.0,
            ChorusOn = ON,
            CreativeChorus = 40.0,
            ChorusRate = 0.5,
            ChorusDepth = 50.0,
            DelayDivision = DIV_4D,
            DelayLevel = 20.0,
            DelayMod = 40.0,
            CreativeDelay = 35.0,
            ReverbLevel = 40.0,
            ReverbDecay = 1.5,
            CreativeReverb = 30.0,
            OutComp = 30.0
        ),
        snapshot_b: vals!(Glitch = 55.0),
        macros: [VERSE_CHORUS, CREATIVE_MACROS[2], WOBBLE, GRIT],
    },
];

/// Factory categories by module, in library order.
pub const CATEGORIES: [&str; 5] = ["Natural", "Tune", "Creative", "Harmony", "Vocoder"];

/// Factory categories by style (finished sounds), in library order.
pub const STYLES: [&str; 8] = [
    "Pop",
    "R&B",
    "Trap",
    "Electro",
    "Choir",
    "Robot",
    "Cinematic",
    "Lo-Fi",
];

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
        for c in STYLES {
            assert!(
                FACTORY.iter().filter(|f| f.category == c).count() >= 2,
                "{c}"
            );
        }
        for (i, f) in FACTORY.iter().enumerate() {
            assert!(
                CATEGORIES.contains(&f.category) || STYLES.contains(&f.category),
                "{}",
                f.name
            );
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
        // Snapshot B: sound parameters only, in range, each a change.
        for f in FACTORY {
            let a = f.values();
            for &(id, v) in f.snapshot_b {
                assert!(crate::snapshot::in_snapshot(id), "{}: {id:?}", f.name);
                assert_eq!(id.spec().kind.sanitize(v), v, "{}: {id:?}", f.name);
                assert_ne!(a.get(id), v, "{}: {id:?} same in A and B", f.name);
            }
            // A preset made to morph has the macro that does it, and
            // only such a preset.
            let morphs = f
                .macros
                .iter()
                .any(|m| m.targets.iter().any(|t| t.0 == P::Morph));
            assert_eq!(morphs, f.snapshots().is_some(), "{}", f.name);
        }
        // The style presets: finished sounds, with their effects.
        let styles: Vec<_> = FACTORY
            .iter()
            .filter(|f| STYLES.contains(&f.category))
            .collect();
        assert!(styles.len() >= 20);
        for f in styles {
            assert!(!f.note.is_empty(), "{}", f.name);
            assert_eq!(f.values().get(P::FxOn), 1.0, "{}", f.name);
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
    fn snapshots_travel_with_the_preset() {
        let i = FACTORY.iter().position(|f| f.name == "Radio Pop").unwrap();
        let p = FACTORY[i].to_preset();
        assert!(p.has_snapshots());
        let b = p.snapshots[1].unwrap();
        assert_eq!(b.get(P::HarmonyOn), 1.0);
        assert_eq!(p.snapshots[0].unwrap(), p.values);
        // Text round trip.
        let q = Preset::parse(&p.to_text()).unwrap();
        assert_eq!(q.snapshots, p.snapshots);
        // Recall stores them; a preset without clears them.
        let c = ControlState::default();
        p.recall_macros(&c);
        assert_eq!(c.snapshot(1).unwrap().get(P::Double), 45.0);
        FACTORY[1].to_preset().recall_macros(&c);
        assert!(!c.has_snapshot(0) && !c.has_snapshot(1));
        // The audio-thread recall (program change) too.
        c.recall_factory_macros(i);
        assert!(c.has_snapshot(0) && c.has_snapshot(1));
        c.recall_factory_macros(1);
        assert!(!c.has_snapshot(1));
        // A user preset captures the stored snapshots.
        c.recall_factory_macros(i);
        let u = Preset::capture("u", "User", &p.values, &c);
        assert_eq!(u.snapshots, p.snapshots);
    }

    #[test]
    fn style_presets_play_cleanly() {
        // A sung vowel (harmonics of 196 Hz, slow vibrato) through each
        // style preset, at A and at B: no blow-up, not silent.
        let sr = 48_000.0;
        let n = 48_000;
        let voice: Vec<f32> = (0..n)
            .map(|i| {
                let t = f64::from(i) / sr;
                let f = 196.0 * (1.0 + 0.004 * (t * 5.0 * std::f64::consts::TAU).sin());
                let ph = t * f * std::f64::consts::TAU;
                #[allow(clippy::cast_possible_truncation)]
                let x = (0.25 * ph.sin()
                    + 0.12 * (2.0 * ph).sin()
                    + 0.08 * (3.0 * ph).sin()
                    + 0.04 * (5.0 * ph).sin()) as f32;
                x
            })
            .collect();
        for f in FACTORY.iter().filter(|f| STYLES.contains(&f.category)) {
            let sets = f
                .snapshots()
                .map_or_else(|| vec![f.values()], |[a, b]| vec![a, b]);
            for v in sets {
                let mut e = crate::engine::Engine::new();
                e.prepare(sr, 64);
                e.set_all(&v, true);
                let (mut peak, mut energy) = (0.0f32, 0.0f64);
                for chunk in voice.chunks(64) {
                    let mut l = chunk.to_vec();
                    let mut r = chunk.to_vec();
                    e.process(&mut l, &mut r);
                    for x in l.iter().chain(&r) {
                        assert!(x.is_finite(), "{}", f.name);
                        peak = peak.max(x.abs());
                        energy += f64::from(*x) * f64::from(*x);
                    }
                }
                assert!(peak < 2.0, "{}: peak {peak}", f.name);
                assert!(energy > 1.0, "{}: silent ({energy})", f.name);
            }
        }
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
