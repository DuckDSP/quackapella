//! Host parameters, mirroring `quack_dsp::params` (the source of truth).
//! Ids are literals because the derive needs them; the adapter tests
//! check id, name, range and default against the registry.

use std::sync::Arc;

use quack_dsp::ParamId;
use quack_dsp::control::ControlState;
use quack_dsp::params::ParamValues;
use truce::core::custom_state::{PersistField, StateCursor, StateField};
use truce::prelude::*;
use truce_params::FloatParamReadF64;

use crate::shared::Shared;

/// Correction styles, in `quack_dsp::VoiceMode` order.
#[derive(ParamEnum)]
pub enum ModeParam {
    #[name = "Natural"]
    Natural,
    #[name = "Tune"]
    Tune,
}

/// Voice ranges, in `quack_dsp::latency::VoiceRange` order.
#[derive(ParamEnum)]
pub enum VoiceParam {
    #[name = "Soprano / Alto"]
    High,
    #[name = "Tenor"]
    Tenor,
    #[name = "Baritone"]
    Baritone,
    #[name = "Bass"]
    Bass,
}

/// Keys, C first.
#[derive(ParamEnum)]
pub enum KeyParam {
    #[name = "C"]
    C,
    #[name = "C#"]
    Cs,
    #[name = "D"]
    D,
    #[name = "D#"]
    Ds,
    #[name = "E"]
    E,
    #[name = "F"]
    F,
    #[name = "F#"]
    Fs,
    #[name = "G"]
    G,
    #[name = "G#"]
    Gs,
    #[name = "A"]
    A,
    #[name = "A#"]
    As,
    #[name = "B"]
    B,
}

/// Scales, in `quack_dsp::autotune::Scale` order.
#[derive(ParamEnum)]
pub enum ScaleParam {
    #[name = "Chromatic"]
    Chromatic,
    #[name = "Major"]
    Major,
    #[name = "Minor"]
    NaturalMinor,
    #[name = "Harmonic minor"]
    HarmonicMinor,
    #[name = "Melodic minor"]
    MelodicMinor,
    #[name = "Dorian"]
    Dorian,
    #[name = "Mixolydian"]
    Mixolydian,
    #[name = "Major pentatonic"]
    MajorPentatonic,
    #[name = "Minor pentatonic"]
    MinorPentatonic,
    #[name = "Blues"]
    Blues,
}

/// Target sources, in `quack_dsp::autotune::Source` order.
#[derive(ParamEnum)]
pub enum SourceParam {
    #[name = "Scale"]
    Scale,
    #[name = "MIDI"]
    Midi,
    #[name = "MIDI Exact"]
    MidiExact,
}

/// Vocoder modes, in `quack_dsp::vocoder::VocoderMode` order.
#[derive(ParamEnum)]
pub enum VocoderParam {
    #[name = "Vocoder"]
    Vocoder,
    #[name = "Talkbox"]
    Talkbox,
}

/// Carriers, in `quack_dsp::vocoder::Carrier` order.
#[derive(ParamEnum)]
pub enum CarrierParam {
    #[name = "Saw"]
    Saw,
    #[name = "Square"]
    Square,
    #[name = "Supersaw"]
    Supersaw,
    #[name = "Noise"]
    Noise,
    #[name = "Sidechain"]
    Sidechain,
    #[name = "PWM"]
    Pulse,
}

/// Band counts, in `quack_dsp::vocoder::BAND_CHOICES` order.
#[derive(ParamEnum)]
pub enum BandsParam {
    #[name = "8"]
    B8,
    #[name = "16"]
    B16,
    #[name = "24"]
    B24,
    #[name = "32"]
    B32,
}

/// Carrier octave, -2 to +2.
#[derive(ParamEnum)]
pub enum OctaveParam {
    #[name = "-2"]
    Down2,
    #[name = "-1"]
    Down1,
    #[name = "0"]
    Zero,
    #[name = "+1"]
    Up1,
    #[name = "+2"]
    Up2,
}

/// Follow chords, in `quack_dsp::vocoder::FollowChord` order.
#[derive(ParamEnum)]
pub enum FollowChordParam {
    #[name = "Single"]
    Single,
    #[name = "Octave"]
    Octave,
    #[name = "Fifth"]
    Fifth,
    #[name = "Major"]
    Major,
    #[name = "Minor"]
    Minor,
    #[name = "Scale"]
    Scale,
}

/// Harmony sources, in `quack_dsp::harmony::HarmonyMode` order.
#[derive(ParamEnum)]
pub enum HarmonyParam {
    #[name = "MIDI"]
    Midi,
    #[name = "Fixed"]
    Fixed,
    #[name = "Scale"]
    Scale,
    #[name = "Chord"]
    Chord,
}

/// Harmony intervals in semitones, -24 to +24, in
/// `quack_dsp::harmony::interval_semitones` order.
#[derive(ParamEnum)]
pub enum IntervalParam {
    #[name = "-24 st"]
    Down24,
    #[name = "-23 st"]
    Down23,
    #[name = "-22 st"]
    Down22,
    #[name = "-21 st"]
    Down21,
    #[name = "-20 st"]
    Down20,
    #[name = "-19 st"]
    Down19,
    #[name = "-18 st"]
    Down18,
    #[name = "-17 st"]
    Down17,
    #[name = "-16 st"]
    Down16,
    #[name = "-15 st"]
    Down15,
    #[name = "-14 st"]
    Down14,
    #[name = "-13 st"]
    Down13,
    #[name = "-12 st"]
    Down12,
    #[name = "-11 st"]
    Down11,
    #[name = "-10 st"]
    Down10,
    #[name = "-9 st"]
    Down9,
    #[name = "-8 st"]
    Down8,
    #[name = "-7 st"]
    Down7,
    #[name = "-6 st"]
    Down6,
    #[name = "-5 st"]
    Down5,
    #[name = "-4 st"]
    Down4,
    #[name = "-3 st"]
    Down3,
    #[name = "-2 st"]
    Down2,
    #[name = "-1 st"]
    Down1,
    #[name = "Off"]
    Off,
    #[name = "+1 st"]
    Up1,
    #[name = "+2 st"]
    Up2,
    #[name = "+3 st"]
    Up3,
    #[name = "+4 st"]
    Up4,
    #[name = "+5 st"]
    Up5,
    #[name = "+6 st"]
    Up6,
    #[name = "+7 st"]
    Up7,
    #[name = "+8 st"]
    Up8,
    #[name = "+9 st"]
    Up9,
    #[name = "+10 st"]
    Up10,
    #[name = "+11 st"]
    Up11,
    #[name = "+12 st"]
    Up12,
    #[name = "+13 st"]
    Up13,
    #[name = "+14 st"]
    Up14,
    #[name = "+15 st"]
    Up15,
    #[name = "+16 st"]
    Up16,
    #[name = "+17 st"]
    Up17,
    #[name = "+18 st"]
    Up18,
    #[name = "+19 st"]
    Up19,
    #[name = "+20 st"]
    Up20,
    #[name = "+21 st"]
    Up21,
    #[name = "+22 st"]
    Up22,
    #[name = "+23 st"]
    Up23,
    #[name = "+24 st"]
    Up24,
}

#[derive(ParamEnum)]
pub enum GlitchRateParam {
    #[name = "1/4"]
    Quarter,
    #[name = "1/8"]
    Eighth,
    #[name = "1/16"]
    Sixteenth,
    #[name = "1/32"]
    ThirtySecond,
}

#[derive(ParamEnum)]
pub enum GlitchModeParam {
    #[name = "Repeat"]
    Repeat,
    #[name = "Stutter"]
    Stutter,
    #[name = "Reverse"]
    Reverse,
    #[name = "Tape"]
    Tape,
}

#[derive(ParamEnum)]
pub enum RoutingParam {
    #[name = "Parallel"]
    Parallel,
    #[name = "Series"]
    Series,
}

#[derive(ParamEnum)]
pub enum ChainParam {
    #[name = "Creative > Harmony > Vocoder"]
    CreativeHarmonyVocoder,
    #[name = "Harmony > Creative > Vocoder"]
    HarmonyCreativeVocoder,
    #[name = "Creative > Vocoder > Harmony"]
    CreativeVocoderHarmony,
    #[name = "Harmony > Vocoder > Creative"]
    HarmonyVocoderCreative,
    #[name = "Vocoder > Creative > Harmony"]
    VocoderCreativeHarmony,
    #[name = "Vocoder > Harmony > Creative"]
    VocoderHarmonyCreative,
}

#[derive(ParamEnum)]
pub enum DivisionParam {
    #[name = "1/16"]
    Sixteenth,
    #[name = "1/8T"]
    EighthTriplet,
    #[name = "1/8"]
    Eighth,
    #[name = "1/4T"]
    QuarterTriplet,
    #[name = "1/8D"]
    DottedEighth,
    #[name = "1/4"]
    Quarter,
    #[name = "1/2T"]
    HalfTriplet,
    #[name = "1/4D"]
    DottedQuarter,
    #[name = "1/2"]
    Half,
    #[name = "1 Bar"]
    Bar,
}

/// Notes that recall the snapshots, in `quack_dsp::snapshot::SnapshotKeys` order.
#[derive(ParamEnum)]
pub enum SnapshotKeysParam {
    #[name = "Off"]
    Off,
    #[name = "C-1 / C#-1"]
    Cm1,
    #[name = "C0 / C#0"]
    C0,
    #[name = "C1 / C#1"]
    C1,
    #[name = "C7 / C#7"]
    C7,
    #[name = "C8 / C#8"]
    C8,
}

/// MIDI output channel.
#[derive(ParamEnum)]
pub enum ChannelParam {
    #[name = "1"]
    Ch1,
    #[name = "2"]
    Ch2,
    #[name = "3"]
    Ch3,
    #[name = "4"]
    Ch4,
    #[name = "5"]
    Ch5,
    #[name = "6"]
    Ch6,
    #[name = "7"]
    Ch7,
    #[name = "8"]
    Ch8,
    #[name = "9"]
    Ch9,
    #[name = "10"]
    Ch10,
    #[name = "11"]
    Ch11,
    #[name = "12"]
    Ch12,
    #[name = "13"]
    Ch13,
    #[name = "14"]
    Ch14,
    #[name = "15"]
    Ch15,
    #[name = "16"]
    Ch16,
}

/// What the MIDI output plays, in `quack_dsp::midiout::OutSource` order.
#[derive(ParamEnum)]
pub enum OutSourceParam {
    #[name = "Sung"]
    Sung,
    #[name = "Sung + Bend"]
    SungBend,
    #[name = "Heard"]
    Heard,
    #[name = "Heard + Harmony"]
    HeardHarmony,
}

#[derive(Params)]
pub struct QuackParams {
    #[param(
        id = 0,
        name = "Input Gain",
        range = "linear(-24, 24)",
        default = 0.0,
        unit = "dB",
        smooth = "none"
    )]
    pub input_gain: FloatParam,

    #[param(
        id = 1,
        name = "Output Gain",
        range = "linear(-24, 24)",
        default = 0.0,
        unit = "dB",
        smooth = "none"
    )]
    pub output_gain: FloatParam,

    #[param(
        id = 2,
        name = "Mix",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub mix: FloatParam,

    #[param(id = 3, name = "Style", default = 0)]
    pub mode: EnumParam<ModeParam>,

    #[param(id = 4, name = "Bypass", flags = "automatable | bypass")]
    pub bypass: BoolParam,

    #[param(
        id = 200,
        name = "Pitch",
        range = "linear(-24, 24)",
        default = 0.0,
        unit = "st",
        smooth = "none"
    )]
    pub pitch: FloatParam,

    #[param(
        id = 201,
        name = "Fine",
        range = "linear(-100, 100)",
        default = 0.0,
        unit = "none",
        smooth = "none"
    )]
    pub fine: FloatParam,

    #[param(
        id = 202,
        name = "Formant",
        range = "linear(-12, 12)",
        default = 0.0,
        unit = "st",
        smooth = "none"
    )]
    pub formant: FloatParam,

    #[param(
        id = 300,
        name = "Correction",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub correction: FloatParam,

    #[param(
        id = 301,
        name = "Retune",
        range = "linear(0, 400)",
        default = 50.0,
        unit = "ms",
        smooth = "none"
    )]
    pub retune: FloatParam,

    #[param(
        id = 302,
        name = "Humanize",
        range = "linear(0, 100)",
        default = 20.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub humanize: FloatParam,

    #[param(
        id = 303,
        name = "Vibrato",
        range = "linear(0, 100)",
        default = 80.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub vibrato: FloatParam,

    #[param(id = 304, name = "Key", default = 0)]
    pub key: EnumParam<KeyParam>,

    #[param(id = 305, name = "Scale", default = 0)]
    pub scale: EnumParam<ScaleParam>,

    #[param(id = 306, name = "Source", default = 0)]
    pub source: EnumParam<SourceParam>,

    #[param(id = 307, name = "Note C", default = true)]
    pub note_c: BoolParam,

    #[param(id = 308, name = "Note C#", default = true)]
    pub note_cs: BoolParam,

    #[param(id = 309, name = "Note D", default = true)]
    pub note_d: BoolParam,

    #[param(id = 310, name = "Note D#", default = true)]
    pub note_ds: BoolParam,

    #[param(id = 311, name = "Note E", default = true)]
    pub note_e: BoolParam,

    #[param(id = 312, name = "Note F", default = true)]
    pub note_f: BoolParam,

    #[param(id = 313, name = "Note F#", default = true)]
    pub note_fs: BoolParam,

    #[param(id = 314, name = "Note G", default = true)]
    pub note_g: BoolParam,

    #[param(id = 315, name = "Note G#", default = true)]
    pub note_gs: BoolParam,

    #[param(id = 316, name = "Note A", default = true)]
    pub note_a: BoolParam,

    #[param(id = 317, name = "Note A#", default = true)]
    pub note_as: BoolParam,

    #[param(id = 318, name = "Note B", default = true)]
    pub note_b: BoolParam,

    #[param(id = 319, name = "MIDI Gate")]
    pub midi_gate: BoolParam,

    #[param(
        id = 320,
        name = "Transition",
        range = "linear(0, 300)",
        default = 0.0,
        unit = "ms",
        smooth = "none"
    )]
    pub transition: FloatParam,

    #[param(
        id = 600,
        name = "Formant Follow",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub formant_follow: FloatParam,

    #[param(
        id = 601,
        name = "Octave Down",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub octave_down: FloatParam,

    #[param(
        id = 602,
        name = "Sub",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub sub: FloatParam,

    #[param(
        id = 603,
        name = "Octave Up",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub octave_up: FloatParam,

    #[param(id = 604, name = "Freeze")]
    pub freeze: BoolParam,

    #[param(
        id = 605,
        name = "Robot",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub robot: FloatParam,

    #[param(
        id = 606,
        name = "Glitch",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub glitch: FloatParam,

    #[param(id = 607, name = "Glitch Rate", default = 2)]
    pub glitch_rate: EnumParam<GlitchRateParam>,

    #[param(id = 608, name = "Glitch Mode", default = 0)]
    pub glitch_mode: EnumParam<GlitchModeParam>,

    #[param(id = 609, name = "Glitch Hold")]
    pub glitch_hold: BoolParam,

    /// Lowest note of the voice: sets the latency.
    #[param(id = 100, name = "Voice", default = 1)]
    pub voice: EnumParam<VoiceParam>,

    /// Shorter grain attacks: less latency, formants a little less
    /// faithful.
    #[param(id = 101, name = "Low Latency")]
    pub low_latency: BoolParam,

    #[param(id = 400, name = "Harmony", default = 2)]
    pub harmony: EnumParam<HarmonyParam>,

    #[param(id = 401, name = "Harmony 1", default = 28)]
    pub harmony_1: EnumParam<IntervalParam>,

    #[param(id = 402, name = "Harmony 2", default = 19)]
    pub harmony_2: EnumParam<IntervalParam>,

    #[param(id = 403, name = "Harmony 3", default = 24)]
    pub harmony_3: EnumParam<IntervalParam>,

    #[param(id = 404, name = "Harmony 4", default = 24)]
    pub harmony_4: EnumParam<IntervalParam>,

    #[param(
        id = 405,
        name = "Lead",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub lead: FloatParam,

    #[param(
        id = 406,
        name = "Harmony Level",
        range = "linear(0, 100)",
        default = 80.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub harmony_level: FloatParam,

    #[param(
        id = 407,
        name = "Spread",
        range = "linear(0, 100)",
        default = 60.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub spread: FloatParam,

    #[param(
        id = 408,
        name = "Harmony Humanize",
        range = "linear(0, 100)",
        default = 30.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub harmony_humanize: FloatParam,

    #[param(
        id = 409,
        name = "Character",
        range = "linear(0, 100)",
        default = 25.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub character: FloatParam,

    #[param(
        id = 410,
        name = "Expression",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub expression: FloatParam,

    #[param(
        id = 411,
        name = "Glide",
        range = "linear(0, 500)",
        default = 30.0,
        unit = "ms",
        smooth = "none"
    )]
    pub glide: FloatParam,

    #[param(
        id = 412,
        name = "Double",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub double: FloatParam,

    #[param(id = 413, name = "Close Voicing")]
    pub close_voicing: BoolParam,

    #[param(id = 414, name = "Latch")]
    pub latch: BoolParam,

    /// Vocoder or talkbox (Synth mode).
    #[param(id = 500, name = "Vocoder")]
    pub vocoder: EnumParam<VocoderParam>,

    #[param(id = 501, name = "Carrier")]
    pub carrier: EnumParam<CarrierParam>,

    #[param(id = 502, name = "Bands", default = 2)]
    pub bands: EnumParam<BandsParam>,

    #[param(
        id = 503,
        name = "Vocoder Level",
        range = "linear(0, 100)",
        default = 80.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub vocoder_level: FloatParam,

    #[param(
        id = 504,
        name = "Response",
        range = "linear(2, 100)",
        default = 12.0,
        unit = "ms",
        smooth = "none"
    )]
    pub response: FloatParam,

    #[param(
        id = 505,
        name = "Band Shift",
        range = "linear(-12, 12)",
        default = 0.0,
        unit = "st",
        smooth = "none"
    )]
    pub band_shift: FloatParam,

    #[param(
        id = 506,
        name = "Sibilance",
        range = "linear(0, 100)",
        default = 50.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub sibilance: FloatParam,

    #[param(id = 507, name = "Carrier Octave", default = 2)]
    pub carrier_octave: EnumParam<OctaveParam>,

    #[param(
        id = 508,
        name = "Detune",
        range = "linear(0, 50)",
        default = 12.0,
        unit = "none",
        smooth = "none"
    )]
    pub detune: FloatParam,

    #[param(
        id = 509,
        name = "Carrier Noise",
        range = "linear(0, 100)",
        default = 5.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub carrier_noise: FloatParam,

    #[param(
        id = 510,
        name = "Carrier Release",
        range = "linear(5, 1000)",
        default = 80.0,
        unit = "ms",
        smooth = "none"
    )]
    pub carrier_release: FloatParam,

    /// Without MIDI, the carrier sings the tuned sung note.
    #[param(id = 511, name = "Carrier Follow", default = true)]
    pub carrier_follow: BoolParam,

    #[param(
        id = 512,
        name = "Carrier Glide",
        range = "linear(0, 500)",
        default = 0.0,
        unit = "ms",
        smooth = "none"
    )]
    pub carrier_glide: FloatParam,

    /// Freezes the vocoder's timbre on the carrier.
    #[param(id = 513, name = "Vocoder Hold")]
    pub vocoder_hold: BoolParam,

    #[param(id = 514, name = "Follow Chord")]
    pub follow_chord: EnumParam<FollowChordParam>,

    #[param(
        id = 515,
        name = "Bend Range",
        range = "linear(0, 12)",
        default = 2.0,
        unit = "st",
        smooth = "none"
    )]
    pub bend_range: FloatParam,

    #[param(
        id = 516,
        name = "Vocoder Width",
        range = "linear(0, 100)",
        default = 50.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub vocoder_width: FloatParam,

    #[param(
        id = 517,
        name = "Vocoder Lo-Fi",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub vocoder_lofi: FloatParam,

    /// Layer switches and the creative layer's depth.
    #[param(id = 610, name = "Creative")]
    pub creative_on: BoolParam,

    #[param(
        id = 611,
        name = "Creative Level",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub creative_level: FloatParam,

    #[param(id = 415, name = "Harmony On")]
    pub harmony_on: BoolParam,

    #[param(id = 518, name = "Vocoder On")]
    pub vocoder_on: BoolParam,

    #[param(
        id = 416,
        name = "Harmony 1 Level",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub harmony_1_level: FloatParam,

    #[param(
        id = 417,
        name = "Harmony 2 Level",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub harmony_2_level: FloatParam,

    #[param(
        id = 418,
        name = "Harmony 3 Level",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub harmony_3_level: FloatParam,

    #[param(
        id = 419,
        name = "Harmony 4 Level",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub harmony_4_level: FloatParam,

    #[param(
        id = 700,
        name = "Macro 1",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub macro_1: FloatParam,

    #[param(
        id = 701,
        name = "Macro 2",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub macro_2: FloatParam,

    #[param(
        id = 702,
        name = "Macro 3",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub macro_3: FloatParam,

    #[param(
        id = 703,
        name = "Macro 4",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        format = "fmt_percent",
        smooth = "none"
    )]
    pub macro_4: FloatParam,

    #[param(id = 800, name = "Routing", default = 0)]
    pub routing: EnumParam<RoutingParam>,

    #[param(id = 801, name = "Chain Order", default = 0)]
    pub chain: EnumParam<ChainParam>,

    #[param(
        id = 1100,
        name = "Throat",
        range = "linear(-100, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub throat: FloatParam,

    #[param(
        id = 1101,
        name = "Throat Pitch",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub throat_pitch: FloatParam,

    #[param(
        id = 1110,
        name = "Harmony Vibrato",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub harmony_vibrato: FloatParam,

    #[param(
        id = 1111,
        name = "Harmony Timing",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub harmony_timing: FloatParam,

    #[param(
        id = 1120,
        name = "Harmony 1 Pan",
        range = "linear(-100, 100)",
        default = -100.0,
        unit = "%",
        smooth = "none"
    )]
    pub harmony_1_pan: FloatParam,

    #[param(
        id = 1121,
        name = "Harmony 2 Pan",
        range = "linear(-100, 100)",
        default = 100.0,
        unit = "%",
        smooth = "none"
    )]
    pub harmony_2_pan: FloatParam,

    #[param(
        id = 1122,
        name = "Harmony 3 Pan",
        range = "linear(-100, 100)",
        default = -50.0,
        unit = "%",
        smooth = "none"
    )]
    pub harmony_3_pan: FloatParam,

    #[param(
        id = 1123,
        name = "Harmony 4 Pan",
        range = "linear(-100, 100)",
        default = 50.0,
        unit = "%",
        smooth = "none"
    )]
    pub harmony_4_pan: FloatParam,

    /// Vocoder enrichments: inputs swapped (the voice by the side-chain),
    /// consonants, resonance, brightness, gate, ensemble, sub.
    #[param(id = 1000, name = "Voice from Sidechain")]
    pub vocoder_swap: BoolParam,

    #[param(
        id = 1001,
        name = "Consonants",
        range = "linear(0, 100)",
        default = 30.0,
        unit = "%",
        smooth = "none"
    )]
    pub consonants: FloatParam,

    #[param(
        id = 1002,
        name = "Vocoder Resonance",
        range = "linear(0, 100)",
        default = 25.0,
        unit = "%",
        smooth = "none"
    )]
    pub vocoder_resonance: FloatParam,

    #[param(
        id = 1003,
        name = "Vocoder Brightness",
        range = "linear(-12, 12)",
        default = 0.0,
        unit = "dB",
        smooth = "none"
    )]
    pub vocoder_tilt: FloatParam,

    #[param(
        id = 1004,
        name = "Vocoder Gate",
        range = "linear(-90, -20)",
        default = -90.0,
        unit = "dB",
        smooth = "none"
    )]
    pub vocoder_gate: FloatParam,

    #[param(
        id = 1005,
        name = "Ensemble",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub ensemble: FloatParam,

    #[param(
        id = 1006,
        name = "Carrier Sub",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub carrier_sub: FloatParam,

    // Voice effects: input dynamics, delay and reverb on sends.
    #[param(id = 900, name = "FX On")]
    pub fx_on: BoolParam,

    #[param(
        id = 901,
        name = "FX Level",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
        smooth = "none"
    )]
    pub fx_level: FloatParam,

    #[param(
        id = 910,
        name = "Gate",
        range = "linear(-80, -20)",
        default = -80.0,
        unit = "dB",
        smooth = "none"
    )]
    pub gate: FloatParam,

    #[param(
        id = 911,
        name = "De-Ess",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub deess: FloatParam,

    #[param(
        id = 912,
        name = "De-Ess Freq",
        range = "linear(3000, 12000)",
        default = 6500.0,
        unit = "Hz",
        smooth = "none"
    )]
    pub deess_freq: FloatParam,

    #[param(
        id = 913,
        name = "Comp Threshold",
        range = "linear(-48, 0)",
        default = -18.0,
        unit = "dB",
        smooth = "none"
    )]
    pub comp_threshold: FloatParam,

    #[param(
        id = 914,
        name = "Comp Ratio",
        range = "linear(1, 20)",
        default = 1.0,
        unit = "none",
        smooth = "none"
    )]
    pub comp_ratio: FloatParam,

    #[param(
        id = 915,
        name = "Comp Attack",
        range = "linear(0.1, 50)",
        default = 5.0,
        unit = "ms",
        smooth = "none"
    )]
    pub comp_attack: FloatParam,

    #[param(
        id = 916,
        name = "Comp Release",
        range = "linear(10, 800)",
        default = 120.0,
        unit = "ms",
        smooth = "none"
    )]
    pub comp_release: FloatParam,

    #[param(
        id = 917,
        name = "Comp Makeup",
        range = "linear(0, 24)",
        default = 0.0,
        unit = "dB",
        smooth = "none"
    )]
    pub comp_makeup: FloatParam,

    #[param(id = 920, name = "Delay Sync", default = true)]
    pub delay_sync: BoolParam,

    #[param(id = 921, name = "Delay Division", default = 4)]
    pub delay_division: EnumParam<DivisionParam>,

    #[param(
        id = 922,
        name = "Delay Time",
        range = "linear(10, 2000)",
        default = 375.0,
        unit = "ms",
        smooth = "none"
    )]
    pub delay_time: FloatParam,

    #[param(
        id = 923,
        name = "Delay Feedback",
        range = "linear(0, 95)",
        default = 35.0,
        unit = "%",
        smooth = "none"
    )]
    pub delay_feedback: FloatParam,

    #[param(id = 924, name = "Delay Ping-Pong")]
    pub delay_ping_pong: BoolParam,

    #[param(
        id = 925,
        name = "Delay Tone",
        range = "linear(0, 100)",
        default = 30.0,
        unit = "%",
        smooth = "none"
    )]
    pub delay_tone: FloatParam,

    #[param(
        id = 926,
        name = "Delay Level",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub delay_level: FloatParam,

    #[param(
        id = 930,
        name = "Reverb Decay",
        range = "linear(0.2, 10)",
        default = 2.0,
        unit = "s",
        smooth = "none"
    )]
    pub reverb_decay: FloatParam,

    #[param(
        id = 931,
        name = "Reverb Damping",
        range = "linear(0, 100)",
        default = 50.0,
        unit = "%",
        smooth = "none"
    )]
    pub reverb_damping: FloatParam,

    #[param(
        id = 932,
        name = "Reverb Pre-Delay",
        range = "linear(0, 200)",
        default = 20.0,
        unit = "ms",
        smooth = "none"
    )]
    pub reverb_predelay: FloatParam,

    #[param(
        id = 933,
        name = "Reverb Level",
        range = "linear(0, 100)",
        default = 60.0,
        unit = "%",
        smooth = "none"
    )]
    pub reverb_level: FloatParam,

    #[param(
        id = 940,
        name = "Duck",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub duck: FloatParam,

    #[param(
        id = 950,
        name = "Send Voice",
        range = "linear(0, 100)",
        default = 15.0,
        unit = "%",
        smooth = "none"
    )]
    pub send_voice: FloatParam,

    #[param(
        id = 951,
        name = "Send Creative",
        range = "linear(0, 100)",
        default = 30.0,
        unit = "%",
        smooth = "none"
    )]
    pub send_creative: FloatParam,

    #[param(
        id = 952,
        name = "Send Harmony",
        range = "linear(0, 100)",
        default = 40.0,
        unit = "%",
        smooth = "none"
    )]
    pub send_harmony: FloatParam,

    #[param(
        id = 953,
        name = "Send Vocoder",
        range = "linear(0, 100)",
        default = 30.0,
        unit = "%",
        smooth = "none"
    )]
    pub send_vocoder: FloatParam,

    #[param(
        id = 1200,
        name = "Morph",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub morph: FloatParam,

    #[param(id = 1201, name = "Morph On")]
    pub morph_on: BoolParam,

    #[param(
        id = 1202,
        name = "Morph Time",
        range = "linear(0, 4000)",
        default = 300.0,
        unit = "ms",
        smooth = "none"
    )]
    pub morph_time: FloatParam,

    #[param(id = 1203, name = "Snapshot Keys", default = 0)]
    pub snapshot_keys: EnumParam<SnapshotKeysParam>,

    #[param(id = 1210, name = "MIDI Out")]
    pub midi_out: BoolParam,

    #[param(id = 1211, name = "MIDI Out Channel", default = 0)]
    pub midi_out_channel: EnumParam<ChannelParam>,

    #[param(id = 1212, name = "MIDI Out Source", default = 0)]
    pub midi_out_source: EnumParam<OutSourceParam>,

    /// Macros, MIDI learn, loaded preset, zoom: saved with the session.
    #[persist]
    pub control: ControlCell,

    /// Audio → editor: held notes, latency. Not a parameter, not saved.
    #[skip]
    pub shared: Arc<Shared>,
}

impl QuackParams {
    /// Display text of the 0-100 percent parameters. truce's built-in
    /// `%` unit expects a 0-1 range and multiplies by 100, which made
    /// hosts show "10000%" (CLAP, VST3).
    #[allow(clippy::unused_self)]
    fn fmt_percent(&self, value: f64) -> String {
        format!("{value:.0}%")
    }

    /// Current host values as the engine's plain values.
    #[must_use]
    pub fn to_values(&self) -> ParamValues {
        let mut v = ParamValues::default();
        v.set(
            ParamId::InputGain,
            FloatParamReadF64::value(&self.input_gain),
        );
        v.set(
            ParamId::OutputGain,
            FloatParamReadF64::value(&self.output_gain),
        );
        v.set(ParamId::Mix, FloatParamReadF64::value(&self.mix));
        #[allow(clippy::cast_precision_loss)]
        v.set(ParamId::Mode, self.mode.value().to_index() as f64);
        v.set(ParamId::Bypass, f64::from(u8::from(self.bypass.value())));
        v.set(ParamId::Pitch, FloatParamReadF64::value(&self.pitch));
        v.set(ParamId::Fine, FloatParamReadF64::value(&self.fine));
        v.set(ParamId::Formant, FloatParamReadF64::value(&self.formant));
        v.set(
            ParamId::Correction,
            FloatParamReadF64::value(&self.correction),
        );
        v.set(ParamId::Retune, FloatParamReadF64::value(&self.retune));
        v.set(ParamId::Humanize, FloatParamReadF64::value(&self.humanize));
        v.set(ParamId::Vibrato, FloatParamReadF64::value(&self.vibrato));
        #[allow(clippy::cast_precision_loss)]
        {
            v.set(ParamId::Key, self.key.value().to_index() as f64);
            v.set(ParamId::Scale, self.scale.value().to_index() as f64);
            v.set(ParamId::Source, self.source.value().to_index() as f64);
        }
        let notes = [
            &self.note_c,
            &self.note_cs,
            &self.note_d,
            &self.note_ds,
            &self.note_e,
            &self.note_f,
            &self.note_fs,
            &self.note_g,
            &self.note_gs,
            &self.note_a,
            &self.note_as,
            &self.note_b,
        ];
        for (id, p) in quack_dsp::params::NOTE_PARAMS.into_iter().zip(notes) {
            v.set(id, f64::from(u8::from(p.value())));
        }
        v.set(
            ParamId::Transition,
            FloatParamReadF64::value(&self.transition),
        );
        v.set(
            ParamId::MidiGate,
            f64::from(u8::from(self.midi_gate.value())),
        );
        for (id, p) in [
            (ParamId::FormantFollow, &self.formant_follow),
            (ParamId::OctaveDown, &self.octave_down),
            (ParamId::Sub, &self.sub),
            (ParamId::OctaveUp, &self.octave_up),
            (ParamId::Robot, &self.robot),
            (ParamId::Glitch, &self.glitch),
        ] {
            v.set(id, FloatParamReadF64::value(p));
        }
        v.set(ParamId::Freeze, f64::from(u8::from(self.freeze.value())));
        v.set(
            ParamId::GlitchHold,
            f64::from(u8::from(self.glitch_hold.value())),
        );
        #[allow(clippy::cast_precision_loss)]
        v.set(
            ParamId::GlitchMode,
            self.glitch_mode.value().to_index() as f64,
        );
        #[allow(clippy::cast_precision_loss)]
        v.set(
            ParamId::GlitchRate,
            self.glitch_rate.value().to_index() as f64,
        );
        #[allow(clippy::cast_precision_loss)]
        v.set(ParamId::Voice, self.voice.value().to_index() as f64);
        v.set(
            ParamId::LowLatency,
            f64::from(u8::from(self.low_latency.value())),
        );
        #[allow(clippy::cast_precision_loss)]
        {
            v.set(ParamId::Harmony, self.harmony.value().to_index() as f64);
            for (id, p) in [
                (ParamId::Harmony1, &self.harmony_1),
                (ParamId::Harmony2, &self.harmony_2),
                (ParamId::Harmony3, &self.harmony_3),
                (ParamId::Harmony4, &self.harmony_4),
            ] {
                v.set(id, p.value().to_index() as f64);
            }
        }
        for (id, p) in [
            (ParamId::Lead, &self.lead),
            (ParamId::HarmonyLevel, &self.harmony_level),
            (ParamId::Spread, &self.spread),
            (ParamId::HarmonyHumanize, &self.harmony_humanize),
            (ParamId::Character, &self.character),
            (ParamId::Expression, &self.expression),
            (ParamId::Glide, &self.glide),
            (ParamId::Double, &self.double),
        ] {
            v.set(id, FloatParamReadF64::value(p));
        }
        v.set(
            ParamId::CloseVoicing,
            f64::from(u8::from(self.close_voicing.value())),
        );
        v.set(ParamId::Latch, f64::from(u8::from(self.latch.value())));
        #[allow(clippy::cast_precision_loss)]
        {
            v.set(ParamId::Vocoder, self.vocoder.value().to_index() as f64);
            v.set(ParamId::Carrier, self.carrier.value().to_index() as f64);
            v.set(ParamId::Bands, self.bands.value().to_index() as f64);
            v.set(
                ParamId::CarrierOctave,
                self.carrier_octave.value().to_index() as f64,
            );
        }
        for (id, p) in [
            (ParamId::VocoderLevel, &self.vocoder_level),
            (ParamId::Response, &self.response),
            (ParamId::BandShift, &self.band_shift),
            (ParamId::Sibilance, &self.sibilance),
            (ParamId::Detune, &self.detune),
            (ParamId::CarrierNoise, &self.carrier_noise),
            (ParamId::CarrierRelease, &self.carrier_release),
        ] {
            v.set(id, FloatParamReadF64::value(p));
        }
        v.set(
            ParamId::CarrierFollow,
            f64::from(u8::from(self.carrier_follow.value())),
        );
        v.set(
            ParamId::VocoderHold,
            f64::from(u8::from(self.vocoder_hold.value())),
        );
        #[allow(clippy::cast_precision_loss)]
        v.set(
            ParamId::FollowChord,
            self.follow_chord.value().to_index() as f64,
        );
        v.set(
            ParamId::CarrierGlide,
            FloatParamReadF64::value(&self.carrier_glide),
        );
        v.set(
            ParamId::BendRange,
            FloatParamReadF64::value(&self.bend_range),
        );
        v.set(
            ParamId::VocoderWidth,
            FloatParamReadF64::value(&self.vocoder_width),
        );
        v.set(
            ParamId::VocoderLofi,
            FloatParamReadF64::value(&self.vocoder_lofi),
        );
        for (id, p) in [
            (ParamId::CreativeOn, &self.creative_on),
            (ParamId::HarmonyOn, &self.harmony_on),
            (ParamId::VocoderOn, &self.vocoder_on),
        ] {
            v.set(id, f64::from(u8::from(p.value())));
        }
        v.set(
            ParamId::CreativeLevel,
            FloatParamReadF64::value(&self.creative_level),
        );
        for (id, p) in quack_dsp::params::HARMONY_LEVEL_PARAMS.into_iter().zip([
            &self.harmony_1_level,
            &self.harmony_2_level,
            &self.harmony_3_level,
            &self.harmony_4_level,
        ]) {
            v.set(id, FloatParamReadF64::value(p));
        }
        for (id, p) in quack_dsp::params::MACRO_PARAMS.into_iter().zip([
            &self.macro_1,
            &self.macro_2,
            &self.macro_3,
            &self.macro_4,
        ]) {
            v.set(id, FloatParamReadF64::value(p));
        }
        #[allow(clippy::cast_precision_loss)]
        {
            v.set(ParamId::Routing, self.routing.value().to_index() as f64);
            v.set(ParamId::Chain, self.chain.value().to_index() as f64);
            v.set(ParamId::Morph, FloatParamReadF64::value(&self.morph));
            v.set(ParamId::MorphOn, f64::from(u8::from(self.morph_on.value())));
            v.set(
                ParamId::MorphTime,
                FloatParamReadF64::value(&self.morph_time),
            );
            v.set(
                ParamId::SnapshotKeys,
                self.snapshot_keys.value().to_index() as f64,
            );
            v.set(ParamId::MidiOut, f64::from(u8::from(self.midi_out.value())));
            v.set(
                ParamId::MidiOutChannel,
                self.midi_out_channel.value().to_index() as f64,
            );
            v.set(
                ParamId::MidiOutSource,
                self.midi_out_source.value().to_index() as f64,
            );
        }
        // Voice effects.
        for (id, p) in [
            (ParamId::FxOn, &self.fx_on),
            (ParamId::DelaySync, &self.delay_sync),
            (ParamId::DelayPingPong, &self.delay_ping_pong),
        ] {
            v.set(id, f64::from(u8::from(p.value())));
        }
        #[allow(clippy::cast_precision_loss)]
        v.set(
            ParamId::DelayDivision,
            self.delay_division.value().to_index() as f64,
        );
        for (id, p) in [
            (ParamId::FxLevel, &self.fx_level),
            (ParamId::Gate, &self.gate),
            (ParamId::DeEss, &self.deess),
            (ParamId::DeEssFreq, &self.deess_freq),
            (ParamId::CompThreshold, &self.comp_threshold),
            (ParamId::CompRatio, &self.comp_ratio),
            (ParamId::CompAttack, &self.comp_attack),
            (ParamId::CompRelease, &self.comp_release),
            (ParamId::CompMakeup, &self.comp_makeup),
            (ParamId::DelayTime, &self.delay_time),
            (ParamId::DelayFeedback, &self.delay_feedback),
            (ParamId::DelayTone, &self.delay_tone),
            (ParamId::DelayLevel, &self.delay_level),
            (ParamId::ReverbDecay, &self.reverb_decay),
            (ParamId::ReverbDamping, &self.reverb_damping),
            (ParamId::ReverbPredelay, &self.reverb_predelay),
            (ParamId::ReverbLevel, &self.reverb_level),
            (ParamId::Duck, &self.duck),
            (ParamId::SendVoice, &self.send_voice),
            (ParamId::SendCreative, &self.send_creative),
            (ParamId::SendHarmony, &self.send_harmony),
            (ParamId::SendVocoder, &self.send_vocoder),
        ] {
            v.set(id, FloatParamReadF64::value(p));
        }
        for (id, p) in [
            (ParamId::Throat, &self.throat),
            (ParamId::ThroatPitch, &self.throat_pitch),
            (ParamId::HarmonyVibrato, &self.harmony_vibrato),
            (ParamId::HarmonyTiming, &self.harmony_timing),
            (ParamId::HarmonyPan1, &self.harmony_1_pan),
            (ParamId::HarmonyPan2, &self.harmony_2_pan),
            (ParamId::HarmonyPan3, &self.harmony_3_pan),
            (ParamId::HarmonyPan4, &self.harmony_4_pan),
        ] {
            v.set(id, FloatParamReadF64::value(p));
        }
        v.set(
            ParamId::VocoderSwap,
            f64::from(u8::from(self.vocoder_swap.value())),
        );
        v.set(
            ParamId::Consonants,
            FloatParamReadF64::value(&self.consonants),
        );
        v.set(
            ParamId::VocoderResonance,
            FloatParamReadF64::value(&self.vocoder_resonance),
        );
        v.set(
            ParamId::VocoderTilt,
            FloatParamReadF64::value(&self.vocoder_tilt),
        );
        v.set(
            ParamId::VocoderGate,
            FloatParamReadF64::value(&self.vocoder_gate),
        );
        v.set(ParamId::Ensemble, FloatParamReadF64::value(&self.ensemble));
        v.set(
            ParamId::CarrierSub,
            FloatParamReadF64::value(&self.carrier_sub),
        );
        v
    }

    /// What the engine plays: the host values with the macros applied.
    #[must_use]
    pub fn engine_values(&self) -> ParamValues {
        let mut v = self.to_values();
        self.control.apply_macros(&mut v);
        v
    }

    /// Recall factory preset `i` from the audio thread (MIDI program
    /// change): atomics only, the setup parameters stay. Returns
    /// whether it exists.
    pub fn recall_factory(&self, i: usize) -> bool {
        let Some(f) = quack_dsp::presets::FACTORY.get(i) else {
            return false;
        };
        let values = f.values();
        for id in ParamId::ALL {
            if quack_dsp::presets::in_preset(id) {
                let spec = id.spec();
                self.set_normalized(spec.host_id, spec.kind.normalize(values.get(id)));
            }
        }
        self.control.recall_factory_macros(i);
        true
    }
}

/// [`ControlState`] as a saved field of the host state: written as the
/// text of [`ControlState::to_text`], keyed by parameter key.
#[derive(Debug, Default)]
pub struct ControlCell(pub ControlState);

impl std::ops::Deref for ControlCell {
    type Target = ControlState;
    fn deref(&self) -> &ControlState {
        &self.0
    }
}

impl PersistField for ControlCell {
    fn persist_write(&self, buf: &mut Vec<u8>) {
        self.0.to_text().write_field(buf);
    }

    fn persist_read(&self, cursor: &mut StateCursor) {
        if let Some(text) = String::read_field(cursor) {
            self.0.load_text(&text);
        }
    }
}
