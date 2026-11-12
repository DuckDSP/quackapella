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

/// Voice modes, in `quack_dsp::VoiceMode` order.
#[derive(ParamEnum)]
pub enum ModeParam {
    #[name = "Natural"]
    Natural,
    #[name = "Tune"]
    Tune,
    #[name = "Creative"]
    Creative,
    #[name = "Synth"]
    Synth,
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
    #[name = "Off"]
    Off,
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
    #[name = "Off"]
    Off,
    #[name = "MIDI"]
    Midi,
    #[name = "Scale"]
    Scale,
    #[name = "Chord"]
    Chord,
}

/// Harmony intervals in scale steps, -9 (a tenth down) to +9, in
/// `quack_dsp::harmony::interval_steps` order.
#[derive(ParamEnum)]
pub enum IntervalParam {
    #[name = "10th down"]
    Down9,
    #[name = "9th down"]
    Down8,
    #[name = "Octave down"]
    Down7,
    #[name = "7th down"]
    Down6,
    #[name = "6th down"]
    Down5,
    #[name = "5th down"]
    Down4,
    #[name = "4th down"]
    Down3,
    #[name = "3rd down"]
    Down2,
    #[name = "2nd down"]
    Down1,
    #[name = "Off"]
    Off,
    #[name = "2nd up"]
    Up1,
    #[name = "3rd up"]
    Up2,
    #[name = "4th up"]
    Up3,
    #[name = "5th up"]
    Up4,
    #[name = "6th up"]
    Up5,
    #[name = "7th up"]
    Up6,
    #[name = "Octave up"]
    Up7,
    #[name = "9th up"]
    Up8,
    #[name = "10th up"]
    Up9,
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
        smooth = "none"
    )]
    pub mix: FloatParam,

    #[param(id = 3, name = "Mode", default = 0)]
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
        smooth = "none"
    )]
    pub humanize: FloatParam,

    #[param(
        id = 303,
        name = "Vibrato",
        range = "linear(0, 100)",
        default = 80.0,
        unit = "%",
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
        smooth = "none"
    )]
    pub formant_follow: FloatParam,

    #[param(
        id = 601,
        name = "Octave Down",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub octave_down: FloatParam,

    #[param(
        id = 602,
        name = "Sub",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub sub: FloatParam,

    #[param(
        id = 603,
        name = "Octave Up",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
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
        smooth = "none"
    )]
    pub robot: FloatParam,

    #[param(
        id = 606,
        name = "Glitch",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
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

    #[param(id = 401, name = "Harmony 1", default = 11)]
    pub harmony_1: EnumParam<IntervalParam>,

    #[param(id = 402, name = "Harmony 2", default = 6)]
    pub harmony_2: EnumParam<IntervalParam>,

    #[param(id = 403, name = "Harmony 3", default = 9)]
    pub harmony_3: EnumParam<IntervalParam>,

    #[param(id = 404, name = "Harmony 4", default = 9)]
    pub harmony_4: EnumParam<IntervalParam>,

    #[param(
        id = 405,
        name = "Lead",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
        smooth = "none"
    )]
    pub lead: FloatParam,

    #[param(
        id = 406,
        name = "Harmony Level",
        range = "linear(0, 100)",
        default = 80.0,
        unit = "%",
        smooth = "none"
    )]
    pub harmony_level: FloatParam,

    #[param(
        id = 407,
        name = "Spread",
        range = "linear(0, 100)",
        default = 60.0,
        unit = "%",
        smooth = "none"
    )]
    pub spread: FloatParam,

    #[param(
        id = 408,
        name = "Harmony Humanize",
        range = "linear(0, 100)",
        default = 30.0,
        unit = "%",
        smooth = "none"
    )]
    pub harmony_humanize: FloatParam,

    #[param(
        id = 409,
        name = "Character",
        range = "linear(0, 100)",
        default = 25.0,
        unit = "%",
        smooth = "none"
    )]
    pub character: FloatParam,

    #[param(
        id = 410,
        name = "Expression",
        range = "linear(0, 100)",
        default = 100.0,
        unit = "%",
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
        smooth = "none"
    )]
    pub vocoder_width: FloatParam,

    #[param(
        id = 517,
        name = "Vocoder Lo-Fi",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub vocoder_lofi: FloatParam,

    #[param(
        id = 700,
        name = "Macro 1",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub macro_1: FloatParam,

    #[param(
        id = 701,
        name = "Macro 2",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub macro_2: FloatParam,

    #[param(
        id = 702,
        name = "Macro 3",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub macro_3: FloatParam,

    #[param(
        id = 703,
        name = "Macro 4",
        range = "linear(0, 100)",
        default = 0.0,
        unit = "%",
        smooth = "none"
    )]
    pub macro_4: FloatParam,

    /// Macros, MIDI learn, loaded preset, zoom: saved with the session.
    #[persist]
    pub control: ControlCell,

    /// Audio → editor: held notes, latency. Not a parameter, not saved.
    #[skip]
    pub shared: Arc<Shared>,
}

impl QuackParams {
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
        for (id, p) in quack_dsp::params::MACRO_PARAMS.into_iter().zip([
            &self.macro_1,
            &self.macro_2,
            &self.macro_3,
            &self.macro_4,
        ]) {
            v.set(id, FloatParamReadF64::value(p));
        }
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
