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
        v
    }
}
