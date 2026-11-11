//! Host parameters, mirroring `quack_dsp::params` (the source of truth).
//! Ids are literals because the derive needs them; the adapter tests
//! check id, name, range and default against the registry.

use std::sync::Arc;

use quack_dsp::ParamId;
use quack_dsp::params::ParamValues;
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
        #[allow(clippy::cast_precision_loss)]
        v.set(
            ParamId::GlitchRate,
            self.glitch_rate.value().to_index() as f64,
        );
        v
    }
}
