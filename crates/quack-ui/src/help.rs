//! Help on hover, shown in the status bar: one line for the control
//! under the pointer, instead of tooltips over the controls.

use egui::Id;
use quack_dsp::ParamId;

/// How long a hint stays after the pointer left its control (seconds).
const HOLD_S: f64 = 0.12;

fn slot() -> Id {
    Id::new("quack-help")
}

fn store(ctx: &egui::Context, text: String) {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(slot(), (now, text)));
}

/// The hint to show this frame, if a control was hovered just now.
pub(crate) fn current(ctx: &egui::Context) -> Option<String> {
    let now = ctx.input(|i| i.time);
    let (at, text): (f64, String) = ctx.data(|d| d.get_temp(slot()))?;
    ((now - at).abs() <= HOLD_S).then_some(text)
}

/// A control's help line: `.hint("...")` on its response.
pub(crate) trait Hint: Sized {
    fn hint(self, text: impl Into<String>) -> Self;
}

impl Hint for egui::Response {
    fn hint(self, text: impl Into<String>) -> Self {
        if self.hovered() {
            let text: String = text.into();
            store(&self.ctx, text.replace('\n', "  \u{b7}  "));
        }
        self
    }
}

/// The help line of a parameter's control (every control calls this
/// through `perform::attach`; a `.hint()` after it wins).
pub(crate) fn param(resp: &egui::Response, id: ParamId) {
    if !resp.hovered() {
        return;
    }
    let name = id.spec().name;
    let what = describe(id);
    store(
        &resp.ctx,
        if what.is_empty() {
            name.to_owned()
        } else {
            format!("{name}: {what}")
        },
    );
}

/// What a parameter does, in one short line (empty: the name says it).
#[allow(clippy::too_many_lines)]
fn describe(id: ParamId) -> &'static str {
    use ParamId as P;
    match id {
        P::InputGain => "level of the voice coming in",
        P::OutputGain => "level going out",
        P::Mix => "dry voice against the processed sound",
        P::Mode => "Natural keeps the singer's gestures, Tune is the hard effect",
        P::Bypass => "the untouched voice, latency kept",
        P::Pitch => "transposition in semitones, formants preserved",
        P::Fine => "transposition in cents",
        P::Formant => "size of the voice, without changing its pitch",
        P::Throat => "one knob from a deeper to a lighter voice",
        P::ThroatPitch => "how much Throat also moves the pitch",
        P::KeepInKey => {
            "transpose before the correction, so the transposed voice stays on the scale (needs Correction and a scale)"
        }
        P::Correction => "how much of the way to the note the voice is pulled",
        P::Retune => "how fast the voice reaches the note (0: hard tune)",
        P::Humanize => "held notes are corrected more gently",
        P::Vibrato => "how much of the singer's vibrato is kept",
        P::Transition => "glide time from one note to the next",
        P::Key => "root of the scale",
        P::Scale => "notes the voice is corrected to",
        P::Source => "target notes: the scale, the MIDI notes held, or the exact MIDI note",
        P::NoteC
        | P::NoteCs
        | P::NoteD
        | P::NoteDs
        | P::NoteE
        | P::NoteF
        | P::NoteFs
        | P::NoteG
        | P::NoteGs
        | P::NoteA
        | P::NoteAs
        | P::NoteB => "click to allow or remove this note",
        P::MidiGate => "MIDI sources: silent while no note is held",
        P::KeyDetect => "Learn finds the key from a few seconds of singing, Auto keeps following",
        P::FormantFollow => "formants follow the transposition (0: preserved)",
        P::OctaveDown => "the voice an octave below",
        P::Sub => "a sub bass under the voice",
        P::OctaveUp => "the voice an octave above",
        P::Freeze => "holds the last sung grain, playable from MIDI",
        P::Robot => "flattens the melody onto one note",
        P::Glitch => "amount of stutter",
        P::GlitchRate => "speed of the stutter",
        P::GlitchMode => "how the stutter is triggered",
        P::GlitchHold => "length of each repeated slice",
        P::Harmony => "how the harmony voices are chosen",
        P::Harmony1 | P::Harmony2 | P::Harmony3 | P::Harmony4 => "interval of this voice",
        P::HarmonyLevel1 | P::HarmonyLevel2 | P::HarmonyLevel3 | P::HarmonyLevel4 => {
            "level of this voice"
        }
        P::HarmonyPan1 | P::HarmonyPan2 | P::HarmonyPan3 | P::HarmonyPan4 => {
            "place of this voice, left to right"
        }
        P::Lead => "level of the lead voice",
        P::HarmonyLevel => "level of the harmony layer",
        P::CreativeLevel => "level of the creative layer",
        P::VocoderLevel => "level of the vocoder layer",
        P::Spread => "stereo width of the harmony voices",
        P::HarmonyHumanize => "small pitch differences between the voices",
        P::HarmonyVibrato => "each voice gets its own vibrato",
        P::HarmonyTiming => "the voices come in slightly apart, like a choir",
        P::Character => "formant of the harmony voices, from darker to lighter",
        P::Expression => "MIDI velocity sets each voice's level",
        P::Glide => "glide between played notes",
        P::Double => "a doubled take of the lead",
        P::CloseVoicing => "MIDI and Chord sources: the voices stay within an octave",
        P::Latch => "played notes hold until the next chord",
        P::Carrier => "what the voice speaks through",
        P::Bands => "number of bands: fewer is more robotic",
        P::Response => "how fast the bands follow the voice",
        P::BandShift => "moves the formants of the vocoded sound",
        P::Sibilance => "consonants passed through for clarity",
        P::Consonants => "level of the consonants in the vocoder",
        P::CarrierOctave => "octave of the built-in synth",
        P::Detune => "detune between the synth's voices",
        P::CarrierNoise => "noise mixed into the carrier",
        P::CarrierSub => "sub oscillator under the carrier",
        P::CarrierRelease => "how long notes ring after release",
        P::CarrierFollow => "without MIDI, the synth follows the sung melody",
        P::CarrierGlide => "glide between synth notes",
        P::VocoderHold => "freezes the vowel",
        P::FollowChord => "the synth plays the detected chord",
        P::BendRange => "pitch bend range in semitones",
        P::VocoderWidth => "stereo width of the vocoder",
        P::VocoderLofi => "vintage grit",
        P::VocoderSwap => "swaps voice and carrier",
        P::VocoderResonance => "sharpness of the bands",
        P::VocoderTilt => "brighter or darker",
        P::VocoderGate => "level under which the vocoder stays silent",
        P::Ensemble => "chorus on the carrier",
        P::Whisper => "a whispered layer under the vocoder",
        P::WhisperTone => "brightness of the whisper",
        P::Macro1 | P::Macro2 | P::Macro3 | P::Macro4 => {
            "right-click any control to assign it to this macro"
        }
        P::Routing => "layers side by side, or one into the next",
        P::Chain => "order of the modules in series",
        P::Gate => "closes under this level",
        P::DeEss | P::OutDeEss => "tames harsh S sounds",
        P::CompThreshold => "level where the compressor starts",
        P::CompRatio => "strength of the compression",
        P::CompMakeup => "level after the compressor",
        P::LowCut | P::EqLowCut => "removes rumble below this frequency",
        P::Presence => "brings the voice forward",
        P::Air => "open top end",
        P::Drive => "amount of saturation",
        P::DriveStyle => "colour of the saturation",
        P::DriveTone => "brightness of the saturation",
        P::DelaySync => "delay time follows the tempo",
        P::DelayFeedback => "number of repeats",
        P::DelayPingPong => "repeats bounce left and right",
        P::DelayDuck => "repeats step back while the voice sings",
        P::DelayToReverb => "sends the repeats into the reverb",
        P::ReverbDecay => "length of the tail",
        P::ReverbPredelay => "gap before the reverb, keeps the voice clear",
        P::ReverbDamping => "how fast the highs die in the tail",
        P::Duck => "the reverb steps back while the voice sings",
        P::OutComp => "glues the whole output",
        P::OutCeiling => "the output never goes above this level",
        P::ExciterAmount => "adds harmonics for presence without harshness",
        P::EqBell1Dyn | P::EqBell2Dyn | P::EqBell3Dyn => "cuts this band only when it builds up",
        P::Morph => "blend between snapshots A and B",
        P::MorphOn => "on: A and B blend with the morph knob. Off: they recall at once",
        P::MorphTime => "time of a recall from A to B",
        P::SnapshotKeys => "MIDI keys that recall the snapshots",
        P::MidiOut => "sends the sung or corrected notes as MIDI",
        P::LinkChannel => "which Quackapella Link group this instance listens to",
        P::SoloVoice | P::SoloCreative | P::SoloHarmony | P::SoloVocoder => "hear this layer alone",
        P::LowLatency => "shortest latency, for live",
        _ => "",
    }
}
