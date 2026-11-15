//! The signal chain.
//!
//! Chain, per channel:
//!
//! ```text
//!                 ┌─ pitch detector (mono, reads only) ─┐ period
//! in ──┬─ in gain ┴─────────── shifter (PSOLA) ─────────┴─┬─ mix ─ out gain ─┬─ bypass fade ─ out
//!      ├──────────────────── dry delay ───────────────────┘                  │
//!      └──────────────────── dry delay ──────────────────────────────────────┘
//! ```
//!
//! The shifter delays the voice by exactly the latency of the voice
//! range ([`crate::latency`]), like the dry delay lines, so dry/wet and
//! bypass never comb and the host's compensation is right (for notes
//! within the range).
//!
//! The autotune ([`Retuner`]) steers the shifter grain by grain: it
//! gets each grain's own period and returns the grain's pitch ratio
//! (correction plus transposition). The harmony voices ([`Harmonizer`])
//! are built on the same analysis grains.
//!
//! Layers: the lead (correction and transposition, level *Lead*), the
//! creative layer (its own signal: a copy of the lead with the robot,
//! formant follow and glitch, plus the octave layers and the freeze;
//! level *Creative Level*), the harmonies and the vocoder. Each has its
//! own switch and level, and they all play together: with the lead
//! down, a layer is heard alone.

use crate::autotune::{Retuner, Scale, Source, TuneReading, TuneSettings, scale_mask};
use crate::chain::{Chain, Routing, Stage, crossfade};
use crate::creative::{Glitch, GlitchMode, GlitchRate, Robot, Steer};
use crate::db_to_gain;
use crate::fx::{
    Chorus, ChorusSettings, Distortion, Division, DriveSettings, DriveStyle, Dynamics,
    DynamicsSettings, Eq, EqSettings, Exciter, ExciterSettings, Output, OutputSettings, Space,
    SpaceSettings, Tone, ToneSettings,
};
use crate::harmony::{Harmonizer, HarmonyMode, INTERVAL_VOICES, interval_semitones};
use crate::key::{KeyDetect, KeyFinder, KeyReading};
use crate::latency::{DelayLine, VoiceRange, latency_samples, max_latency_samples};
use crate::midi::NoteStack;
use crate::mode::VoiceMode;
use crate::params::{NOTE_PARAMS, ParamId, ParamValues};
use crate::pitch::{PitchDetector, PitchEstimate, Yin};
use crate::shift::{Buses, LAYERS, Psola, semitones_to_ratio};

/// Fastest retune in the Natural mode: below it the correction is
/// heard as an effect, which is the Tune mode's job.
pub const NATURAL_MIN_RETUNE_S: f64 = 0.03;
use crate::smooth::Ramp;
use crate::tap::Taps;
use crate::throat::{self, Throat};
use crate::vocoder::{Carrier, FollowChord, Vocoder, VocoderMode, bands_from_index};

/// Smoothing time for gains and mix.
const SMOOTH_MS: f64 = 20.0;
/// Bypass crossfade.
const BYPASS_MS: f64 = 10.0;
/// MIDI Gate fade in and out.
const GATE_MS: f64 = 8.0;

pub const CHANNELS: usize = 2;

#[derive(Debug, Clone)]
pub struct Engine {
    sample_rate: f64,
    latency: usize,
    range: VoiceRange,
    low_latency: bool,
    values: ParamValues,
    mode: VoiceMode,
    in_gain: Ramp,
    out_gain: Ramp,
    mix: Ramp,
    /// 0 = processed, 1 = bypassed.
    bypass: Ramp,
    /// Transposition and formant shift, in semitones.
    pitch: Ramp,
    formant: Ramp,
    /// Throat size (-1 shortest to +1 longest), its pitch link (0 to
    /// 1) and its body and top filters (see [`crate::throat`]).
    throat: Ramp,
    throat_pitch: Ramp,
    throat_eq: Throat,
    dry: [DelayLine; CHANNELS],
    /// Inputs swapped (the voice by the side-chain), and the track's
    /// own signal delayed for the dry path and the bypass then.
    swap: bool,
    track: [DelayLine; CHANNELS],
    notes: NoteStack,
    detector: Yin,
    shifter: Psola,
    retuner: Retuner,
    /// MIDI Gate: 1 = open. Delayed by the latency, so it opens and
    /// closes with the audio the notes were played against.
    gate: Ramp,
    gate_delay: DelayLine,
    /// Creative layer: octave layers (down, sub, up), formant follow,
    /// freeze, robot, glitch. All at rest with the layer off.
    layers: [Ramp; LAYERS],
    follow: Ramp,
    /// The creative layer's level, and its copy of the lead (on while
    /// the robot, formant follow or glitch is used).
    creative_level: Ramp,
    copy: Ramp,
    freeze: bool,
    robot: Robot,
    glitch: Glitch,
    /// Harmony layer: harmony voices, lead and harmony levels.
    harmony: Harmonizer,
    lead: Ramp,
    harmony_level: Ramp,
    /// Vocoder layer: vocoder or talkbox on the internal carrier synth or
    /// the side-chain, and its level.
    vocoder: Vocoder,
    vocoder_level: Ramp,
    /// Solo: each layer's gain (0 while another is soloed), and
    /// whether any solo is on (for the series chain).
    solo: [Ramp; 4],
    soloing: Ramp,
    double: Ramp,
    /// Layer routing and, in series, the order of the modules.
    routing: Routing,
    chain: Chain,
    /// Voice effects: dynamics at the input, delay and reverb on one
    /// bus fed by a send per layer, its return level.
    fx_on: bool,
    dynamics: Dynamics,
    tone: Tone,
    chorus: Chorus,
    space: Space,
    space_on: bool,
    fx_return: Ramp,
    /// Per layer (voice, creative, harmony, vocoder): reverb send,
    /// delay send, chorus amount, drive amount.
    sends: [Ramp; 4],
    delay_sends: [Ramp; 4],
    chorus_amounts: [Ramp; 4],
    drive_amounts: [Ramp; 4],
    drives: [Distortion; 4],
    drive_on: bool,
    output: Output,
    /// Colour of the transformed sound: EQ and exciter after the
    /// layers, before the delay and reverb returns and the output
    /// dynamics.
    eq: Eq,
    exciter: Exciter,
    /// Analysis taps for the editor (off the audio path).
    taps: Taps,
    /// Transposition before the correction (the voice stays in key).
    keep_in_key: bool,
    /// Key detection (Learn / Auto), off the signal path.
    key: KeyFinder,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        let mut e = Self {
            sample_rate: 48_000.0,
            latency: 0,
            range: VoiceRange::default(),
            low_latency: false,
            values: ParamValues::default(),
            mode: VoiceMode::default(),
            in_gain: Ramp::new(1.0),
            out_gain: Ramp::new(1.0),
            mix: Ramp::new(1.0),
            bypass: Ramp::new(0.0),
            pitch: Ramp::new(0.0),
            formant: Ramp::new(0.0),
            throat: Ramp::new(0.0),
            throat_pitch: Ramp::new(0.0),
            throat_eq: Throat::default(),
            dry: Default::default(),
            swap: false,
            track: Default::default(),
            notes: NoteStack::default(),
            detector: Yin::default(),
            shifter: Psola::default(),
            retuner: Retuner::default(),
            gate: Ramp::new(1.0),
            gate_delay: DelayLine::default(),
            layers: [Ramp::new(0.0), Ramp::new(0.0), Ramp::new(0.0)],
            follow: Ramp::new(0.0),
            creative_level: Ramp::new(0.0),
            copy: Ramp::new(0.0),
            freeze: false,
            robot: Robot::default(),
            glitch: Glitch::default(),
            harmony: Harmonizer::default(),
            lead: Ramp::new(1.0),
            harmony_level: Ramp::new(0.0),
            vocoder: Vocoder::default(),
            vocoder_level: Ramp::new(0.0),
            solo: [Ramp::new(1.0); 4],
            soloing: Ramp::new(0.0),
            double: Ramp::new(0.0),
            routing: Routing::Parallel,
            chain: Chain::default(),
            fx_on: false,
            dynamics: Dynamics::default(),
            tone: Tone::default(),
            chorus: Chorus::default(),
            space: Space::default(),
            space_on: false,
            fx_return: Ramp::new(0.0),
            sends: [Ramp::new(0.0); 4],
            delay_sends: [Ramp::new(0.0); 4],
            chorus_amounts: [Ramp::new(0.0); 4],
            drive_amounts: [Ramp::new(0.0); 4],
            drives: Default::default(),
            drive_on: false,
            output: Output::default(),
            eq: Eq::default(),
            exciter: Exciter::default(),
            taps: Taps::default(),
            keep_in_key: false,
            key: KeyFinder::default(),
        };
        e.prepare(48_000.0, 512);
        e
    }

    /// Allocates delay lines and resets state. Not for the audio thread.
    pub fn prepare(&mut self, sample_rate: f64, max_block: usize) {
        self.sample_rate = sample_rate;
        self.key.prepare(sample_rate);
        // Allocated for the lowest range; the voice range picks the delay.
        let max = max_latency_samples(sample_rate);
        for d in self.dry.iter_mut().chain(self.track.iter_mut()) {
            d.prepare(max);
        }
        for r in [
            &mut self.in_gain,
            &mut self.out_gain,
            &mut self.mix,
            &mut self.pitch,
            &mut self.formant,
            &mut self.throat,
            &mut self.throat_pitch,
            &mut self.follow,
            &mut self.lead,
            &mut self.harmony_level,
            &mut self.vocoder_level,
            &mut self.double,
            &mut self.fx_return,
            &mut self.soloing,
        ]
        .into_iter()
        .chain(self.layers.iter_mut())
        .chain(self.solo.iter_mut())
        .chain(self.sends.iter_mut())
        .chain(self.delay_sends.iter_mut())
        .chain(self.chorus_amounts.iter_mut())
        .chain(self.drive_amounts.iter_mut())
        {
            r.prepare(sample_rate, SMOOTH_MS);
        }
        self.bypass.prepare(sample_rate, BYPASS_MS);
        self.gate.prepare(sample_rate, GATE_MS);
        self.gate_delay.prepare(max);
        self.detector.prepare(sample_rate, max_block);
        self.shifter.prepare(sample_rate, max);
        self.retuner.prepare(sample_rate);
        self.throat_eq.prepare(sample_rate);
        self.robot.prepare(sample_rate);
        self.glitch.prepare(sample_rate);
        self.harmony.prepare(sample_rate);
        self.vocoder.prepare(sample_rate, max);
        self.dynamics.prepare(sample_rate);
        self.tone.prepare(sample_rate);
        self.chorus.prepare(sample_rate);
        for d in &mut self.drives {
            d.prepare(sample_rate);
        }
        self.output.prepare(sample_rate);
        self.eq.prepare(sample_rate);
        self.exciter.prepare(sample_rate);
        self.space.prepare(sample_rate);
        self.notes.clear();
        self.latency = max;
        self.range = VoiceRange::from_index(self.values.get(ParamId::Voice));
        self.low_latency = self.values.get(ParamId::LowLatency) >= 0.5;
        self.apply_latency();
        self.apply_all(true);
    }

    /// Latency and grain attack from the voice range and the Low Latency
    /// switch.
    fn apply_latency(&mut self) {
        self.shifter
            .set_attack(crate::latency::attack(self.low_latency));
        self.set_latency(latency_samples(
            self.sample_rate,
            self.range,
            self.low_latency,
        ));
    }

    /// Move every delay to `latency` and clear the audio (the output
    /// timeline moves: the host re-aligns). Real-time safe.
    fn set_latency(&mut self, latency: usize) {
        self.latency = latency;
        for d in self.dry.iter_mut().chain(self.track.iter_mut()) {
            d.set_delay(latency);
        }
        self.gate_delay.set_delay(latency);
        self.shifter.set_latency(latency);
        self.vocoder.set_latency(self.vocoder_latency());
        self.shifter.reset();
        self.prime_gate();
    }

    /// The vocoder's output alignment: in parallel it lines up with the
    /// engine's latency; in series its modulator (the chain so far) is
    /// already that late.
    fn vocoder_latency(&self) -> usize {
        match self.routing {
            Routing::Parallel => self.latency,
            Routing::Series => 0,
        }
    }

    /// Clears audio state (transport jump, host reset). Real-time safe.
    pub fn reset(&mut self) {
        for d in self.dry.iter_mut().chain(self.track.iter_mut()) {
            d.reset();
        }
        self.detector.reset();
        self.shifter.reset();
        self.retuner.reset();
        self.throat_eq.reset();
        self.robot.reset();
        self.glitch.reset();
        self.harmony.reset();
        self.vocoder.reset();
        self.dynamics.reset();
        self.tone.reset();
        self.chorus.reset();
        for d in &mut self.drives {
            d.reset();
        }
        self.output.reset();
        self.eq.reset();
        self.exciter.reset();
        self.space.reset();
        self.notes.clear();
        self.apply_all(true);
        self.prime_gate();
    }

    /// The latency the host must compensate, in samples. Depends on the
    /// sample rate and the voice range only.
    #[must_use]
    pub fn latency_samples(&self) -> usize {
        self.latency
    }

    /// Delay the voiced (shifted) voice really has right now, in samples:
    /// the reported latency plus what a low voice adds.
    #[must_use]
    pub fn voice_latency(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let l = self.latency as f64;
        l + self.shifter.grain_delay()
    }

    #[must_use]
    pub fn mode(&self) -> VoiceMode {
        self.mode
    }

    #[must_use]
    pub fn notes(&self) -> &NoteStack {
        &self.notes
    }

    /// What the autotune is doing (sung, target and output notes),
    /// `None` while unvoiced or with the correction off.
    #[must_use]
    pub fn tune(&self) -> Option<TuneReading> {
        self.retuner.reading()
    }

    /// Allowed pitch classes of the autotune right now (bit 0 = C),
    /// MIDI notes included.
    #[must_use]
    pub fn tune_mask(&self) -> u16 {
        self.retuner.settings.mask
    }

    /// Latest pitch reading of the input, `None` while unvoiced.
    #[must_use]
    pub fn pitch(&self) -> Option<PitchEstimate> {
        self.detector.estimate()
    }

    #[must_use]
    pub fn param(&self, id: ParamId) -> f64 {
        self.values.get(id)
    }

    /// Set a parameter (plain value). Ramps from the current value.
    pub fn set_param(&mut self, id: ParamId, plain: f64) {
        self.values.set(id, plain);
        self.apply(id, false);
    }

    /// Set every parameter; `snap` skips the ramps (first block).
    pub fn set_all(&mut self, values: &ParamValues, snap: bool) {
        self.values = *values;
        self.apply_all(snap);
    }

    fn apply_all(&mut self, snap: bool) {
        for id in ParamId::ALL {
            self.apply(id, snap);
        }
    }

    fn apply(&mut self, id: ParamId, snap: bool) {
        #[allow(clippy::cast_possible_truncation)]
        let v = self.values.get(id) as f32;
        let (ramp, target) = match id {
            ParamId::InputGain => (&mut self.in_gain, db_to_gain(v)),
            ParamId::OutputGain => (&mut self.out_gain, db_to_gain(v)),
            ParamId::Mix => (&mut self.mix, v / 100.0),
            ParamId::Bypass => (&mut self.bypass, v),
            ParamId::Pitch | ParamId::Fine => {
                #[allow(clippy::cast_possible_truncation)]
                let st = (self.values.get(ParamId::Pitch) + self.values.get(ParamId::Fine) / 100.0)
                    as f32;
                (&mut self.pitch, st)
            }
            ParamId::Formant => (&mut self.formant, v),
            ParamId::KeepInKey => {
                self.keep_in_key = v >= 0.5;
                return;
            }
            ParamId::KeyDetect => {
                self.key.set_mode(KeyDetect::from_index(f64::from(v)));
                return;
            }
            ParamId::Throat => (&mut self.throat, v / 100.0),
            ParamId::ThroatPitch => (&mut self.throat_pitch, v / 100.0),
            ParamId::Mode => {
                self.mode = VoiceMode::from_index(f64::from(v));
                self.update_tune();
                return;
            }
            ParamId::CreativeOn
            | ParamId::CreativeLevel
            | ParamId::FormantFollow
            | ParamId::OctaveDown
            | ParamId::Sub
            | ParamId::OctaveUp
            | ParamId::Freeze
            | ParamId::Robot
            | ParamId::Glitch
            | ParamId::GlitchRate
            | ParamId::GlitchMode
            | ParamId::GlitchHold => {
                self.update_creative(snap);
                return;
            }
            ParamId::HarmonyOn
            | ParamId::Harmony
            | ParamId::Harmony1
            | ParamId::Harmony2
            | ParamId::Harmony3
            | ParamId::Harmony4
            | ParamId::Lead
            | ParamId::HarmonyLevel
            | ParamId::Spread
            | ParamId::HarmonyHumanize
            | ParamId::Character
            | ParamId::Expression
            | ParamId::Glide
            | ParamId::Double
            | ParamId::CloseVoicing
            | ParamId::Latch
            | ParamId::HarmonyLevel1
            | ParamId::HarmonyLevel2
            | ParamId::HarmonyLevel3
            | ParamId::HarmonyLevel4
            | ParamId::HarmonyVibrato
            | ParamId::HarmonyTiming
            | ParamId::HarmonyPan1
            | ParamId::HarmonyPan2
            | ParamId::HarmonyPan3
            | ParamId::HarmonyPan4 => {
                self.update_harmony(snap);
                self.update_vocoder(snap);
                return;
            }
            ParamId::VocoderOn
            | ParamId::Vocoder
            | ParamId::Carrier
            | ParamId::Bands
            | ParamId::VocoderLevel
            | ParamId::Response
            | ParamId::BandShift
            | ParamId::Sibilance
            | ParamId::CarrierOctave
            | ParamId::Detune
            | ParamId::CarrierNoise
            | ParamId::CarrierRelease
            | ParamId::CarrierFollow
            | ParamId::CarrierGlide
            | ParamId::VocoderHold
            | ParamId::FollowChord
            | ParamId::BendRange
            | ParamId::VocoderWidth
            | ParamId::VocoderLofi
            | ParamId::VocoderSwap
            | ParamId::Consonants
            | ParamId::VocoderResonance
            | ParamId::VocoderTilt
            | ParamId::VocoderGate
            | ParamId::Ensemble
            | ParamId::CarrierSub
            | ParamId::Whisper
            | ParamId::WhisperTone => {
                self.update_vocoder(snap);
                return;
            }
            ParamId::Routing | ParamId::Chain => {
                let routing = Routing::from_index(f64::from(v));
                let routing = if id == ParamId::Routing {
                    routing
                } else {
                    self.routing
                };
                if id == ParamId::Chain {
                    self.chain = Chain::from_index(f64::from(v));
                }
                if routing != self.routing {
                    // The vocoder's input moves: start it clean.
                    self.routing = routing;
                    self.vocoder.set_latency(self.vocoder_latency());
                    self.vocoder.reset();
                    self.glitch.reset();
                }
                return;
            }
            ParamId::FxOn
            | ParamId::FxLevel
            | ParamId::Gate
            | ParamId::DeEss
            | ParamId::DeEssFreq
            | ParamId::CompThreshold
            | ParamId::CompRatio
            | ParamId::CompAttack
            | ParamId::CompRelease
            | ParamId::CompMakeup
            | ParamId::DelaySync
            | ParamId::DelayDivision
            | ParamId::DelayTime
            | ParamId::DelayFeedback
            | ParamId::DelayPingPong
            | ParamId::DelayTone
            | ParamId::DelayLevel
            | ParamId::ReverbDecay
            | ParamId::ReverbDamping
            | ParamId::ReverbPredelay
            | ParamId::ReverbLevel
            | ParamId::Duck
            | ParamId::DelayDuck
            | ParamId::VoiceReverb
            | ParamId::CreativeReverb
            | ParamId::HarmonyReverb
            | ParamId::VocoderReverb
            | ParamId::DynamicsOn
            | ParamId::ToneOn
            | ParamId::DelayOn
            | ParamId::ReverbOn
            | ParamId::ChorusOn
            | ParamId::LowCut
            | ParamId::Presence
            | ParamId::Air
            | ParamId::Drive
            | ParamId::DelayMod
            | ParamId::DelayWidth
            | ParamId::DelayLowCut
            | ParamId::ReverbSize
            | ParamId::ReverbMod
            | ParamId::ReverbLowCut
            | ParamId::ReverbWidth
            | ParamId::DelayToReverb
            | ParamId::ChorusRate
            | ParamId::ChorusDepth
            | ParamId::OutputOn
            | ParamId::OutDeEss
            | ParamId::OutComp
            | ParamId::OutCeiling
            | ParamId::DriveStyle
            | ParamId::DriveTone
            | ParamId::DriveOn
            | ParamId::VoiceChorus
            | ParamId::CreativeChorus
            | ParamId::HarmonyChorus
            | ParamId::VocoderChorus
            | ParamId::VoiceDelay
            | ParamId::CreativeDelay
            | ParamId::HarmonyDelay
            | ParamId::VocoderDelay
            | ParamId::VoiceDrive
            | ParamId::CreativeDrive
            | ParamId::HarmonyDrive
            | ParamId::VocoderDrive
            | ParamId::EqOn
            | ParamId::EqLowCut
            | ParamId::EqLowShelfFreq
            | ParamId::EqLowShelfGain
            | ParamId::EqBell1Freq
            | ParamId::EqBell1Gain
            | ParamId::EqBell1Q
            | ParamId::EqBell2Freq
            | ParamId::EqBell2Gain
            | ParamId::EqBell2Q
            | ParamId::EqBell3Freq
            | ParamId::EqBell3Gain
            | ParamId::EqBell3Q
            | ParamId::EqHighShelfFreq
            | ParamId::EqHighShelfGain
            | ParamId::ExciterAmount
            | ParamId::ExciterFreq
            | ParamId::EqBell1Dyn
            | ParamId::EqBell2Dyn
            | ParamId::EqBell3Dyn => {
                self.update_fx(snap);
                return;
            }
            ParamId::Voice => {
                let range = VoiceRange::from_index(f64::from(v));
                if range != self.range {
                    self.range = range;
                    self.apply_latency();
                }
                return;
            }
            ParamId::LowLatency => {
                let low = v >= 0.5;
                if low != self.low_latency {
                    self.low_latency = low;
                    self.apply_latency();
                }
                return;
            }
            // Macros act through the values they modulate (the adapter
            // applies them before the engine sees anything).
            ParamId::Macro1 | ParamId::Macro2 | ParamId::Macro3 | ParamId::Macro4 => return,
            // Snapshots, morph and MIDI out live in the adapter: the
            // morph writes the parameters it moves, the MIDI out reads
            // the engine's readings.
            ParamId::Morph
            | ParamId::MorphOn
            | ParamId::MorphTime
            | ParamId::SnapshotKeys
            | ParamId::MidiOut
            | ParamId::MidiOutChannel
            | ParamId::MidiOutSource
            // The Link channel picks the wire, in the adapter.
            | ParamId::LinkChannel => return,
            ParamId::SoloVoice
            | ParamId::SoloCreative
            | ParamId::SoloHarmony
            | ParamId::SoloVocoder => {
                self.update_solo(snap);
                return;
            }
            _ => {
                // Key, scale, note switches: the harmonies count in them.
                self.update_tune();
                self.update_harmony(snap);
                self.update_vocoder(snap);
                return;
            }
        };
        if snap {
            ramp.snap(target);
        } else {
            ramp.set(target);
        }
    }

    /// Voice effects from the parameters. With FX off the dynamics are
    /// neutral and the bus return fades out (its tails with it).
    fn update_fx(&mut self, snap: bool) {
        let v = &self.values;
        #[allow(clippy::cast_possible_truncation)]
        let f = |id: ParamId| v.get(id) as f32;
        let pct = |id: ParamId| f(id) / 100.0;
        let on = v.get(ParamId::FxOn) >= 0.5;
        let sw = |id: ParamId| on && v.get(id) >= 0.5;
        self.tone.set(if sw(ParamId::ToneOn) {
            ToneSettings {
                low_cut_hz: f(ParamId::LowCut),
                presence_db: f(ParamId::Presence),
                air_db: f(ParamId::Air),
            }
        } else {
            ToneSettings::default()
        });
        self.chorus.set(ChorusSettings {
            rate_hz: f(ParamId::ChorusRate),
            depth: pct(ParamId::ChorusDepth),
            mix: if sw(ParamId::ChorusOn) { 1.0 } else { 0.0 },
        });
        let drive = DriveSettings {
            style: DriveStyle::from_index(v.get(ParamId::DriveStyle)),
            drive: pct(ParamId::Drive),
            tone: pct(ParamId::DriveTone),
        };
        for d in &mut self.drives {
            d.set(drive);
        }
        self.drive_on = sw(ParamId::DriveOn);
        let bell = |ids: [ParamId; 3]| (f(ids[0]), f(ids[1]), f(ids[2]));
        let eq_on = sw(ParamId::EqOn);
        self.eq.set(
            EqSettings {
                low_cut_hz: f(ParamId::EqLowCut),
                low_shelf_hz: f(ParamId::EqLowShelfFreq),
                low_shelf_db: f(ParamId::EqLowShelfGain),
                bells: crate::params::EQ_BELL_PARAMS.map(bell),
                dynamic: crate::params::EQ_DYN_PARAMS.map(f),
                high_shelf_hz: f(ParamId::EqHighShelfFreq),
                high_shelf_db: f(ParamId::EqHighShelfGain),
            },
            eq_on,
        );
        self.exciter.set(ExciterSettings {
            amount: if eq_on {
                pct(ParamId::ExciterAmount)
            } else {
                0.0
            },
            hz: f(ParamId::ExciterFreq),
        });
        self.output.set(
            OutputSettings {
                deess: pct(ParamId::OutDeEss),
                comp: pct(ParamId::OutComp),
                ceiling_db: f(ParamId::OutCeiling),
            },
            sw(ParamId::OutputOn),
        );
        self.dynamics.set(if sw(ParamId::DynamicsOn) {
            DynamicsSettings {
                gate_db: f(ParamId::Gate),
                deess: pct(ParamId::DeEss),
                deess_hz: f(ParamId::DeEssFreq),
                threshold_db: f(ParamId::CompThreshold),
                ratio: f(ParamId::CompRatio),
                attack_ms: f(ParamId::CompAttack),
                release_ms: f(ParamId::CompRelease),
                makeup_db: f(ParamId::CompMakeup),
            }
        } else {
            DynamicsSettings::default()
        });
        if on && !self.fx_on {
            // Back on: no tail left over from before.
            self.dynamics.reset();
            self.tone.reset();
            self.chorus.reset();
            for d in &mut self.drives {
                d.reset();
            }
            self.output.reset();
            self.eq.reset();
            self.exciter.reset();
            self.space.reset();
        }
        self.fx_on = on;
        let delay_on = v.get(ParamId::DelayOn) >= 0.5;
        let reverb_on = v.get(ParamId::ReverbOn) >= 0.5;
        self.space.set(SpaceSettings {
            sync: v.get(ParamId::DelaySync) >= 0.5,
            division: Division::from_index(v.get(ParamId::DelayDivision)),
            delay_ms: f(ParamId::DelayTime),
            feedback: pct(ParamId::DelayFeedback),
            ping_pong: v.get(ParamId::DelayPingPong) >= 0.5,
            delay_tone: pct(ParamId::DelayTone),
            delay_level: if delay_on {
                pct(ParamId::DelayLevel)
            } else {
                0.0
            },
            decay_s: f(ParamId::ReverbDecay),
            damping: pct(ParamId::ReverbDamping),
            predelay_ms: f(ParamId::ReverbPredelay),
            reverb_level: if reverb_on {
                pct(ParamId::ReverbLevel)
            } else {
                0.0
            },
            duck: pct(ParamId::Duck),
            delay_duck: pct(ParamId::DelayDuck),
            delay_mod: pct(ParamId::DelayMod),
            delay_width: pct(ParamId::DelayWidth),
            delay_low_cut_hz: f(ParamId::DelayLowCut),
            reverb_size: pct(ParamId::ReverbSize),
            reverb_mod: pct(ParamId::ReverbMod),
            reverb_low_cut_hz: f(ParamId::ReverbLowCut),
            reverb_width: pct(ParamId::ReverbWidth),
            delay_to_reverb: pct(ParamId::DelayToReverb),
        });
        let ret = if on { pct(ParamId::FxLevel) } else { 0.0 };
        let per_layer = |ids: [ParamId; 4]| ids.map(pct);
        let sends = per_layer(crate::params::REVERB_SEND_PARAMS);
        let delays = per_layer(crate::params::DELAY_SEND_PARAMS);
        let choruses = per_layer(crate::params::CHORUS_AMOUNT_PARAMS);
        let drives = per_layer(crate::params::DRIVE_AMOUNT_PARAMS);
        for (r, t) in std::iter::once(&mut self.fx_return)
            .chain(self.sends.iter_mut())
            .chain(self.delay_sends.iter_mut())
            .chain(self.chorus_amounts.iter_mut())
            .chain(self.drive_amounts.iter_mut())
            .zip(
                std::iter::once(ret)
                    .chain(sends)
                    .chain(delays)
                    .chain(choruses)
                    .chain(drives),
            )
        {
            if snap {
                r.snap(t);
            } else {
                r.set(t);
            }
        }
        if on {
            self.space_on = true;
        }
    }

    /// Gain reduction of the input compressor and de-esser, in dB (≤ 0).
    #[must_use]
    pub fn fx_reduction_db(&self) -> (f32, f32) {
        self.dynamics.reduction_db()
    }

    /// The analysis taps (shared rings, see [`crate::tap`]).
    #[must_use]
    pub fn taps(&self) -> Taps {
        self.taps.clone()
    }

    /// Write the taps into rings the editor already holds.
    pub fn set_taps(&mut self, taps: Taps) {
        self.taps = taps;
    }

    /// Gain reduction of the output compressor and de-esser, in dB (≤ 0).
    #[must_use]
    pub fn output_reduction_db(&self) -> (f32, f32) {
        self.output.reduction_db()
    }

    /// What each bell of the output EQ cuts dynamically right now, dB
    /// (≥ 0).
    #[must_use]
    pub fn eq_dynamic_cut_db(&self) -> [f32; crate::fx::EQ_BELLS] {
        self.eq.dynamic_cut_db()
    }

    /// Whether the Creative layer is on.
    fn creative(&self) -> bool {
        self.values.get(ParamId::CreativeOn) >= 0.5
    }

    /// Creative effects from the parameters, nothing with the layer
    /// off. The layer's level scales its whole output.
    fn update_creative(&mut self, snap: bool) {
        let on = self.creative();
        let v = &self.values;
        #[allow(clippy::cast_possible_truncation)]
        let pct = |id: ParamId| if on { (v.get(id) / 100.0) as f32 } else { 0.0 };
        #[allow(clippy::cast_possible_truncation)]
        let level = if on {
            (v.get(ParamId::CreativeLevel) / 100.0) as f32
        } else {
            0.0
        };
        // The copy of the lead carries the effects that transform it.
        let copy = [ParamId::Robot, ParamId::FormantFollow, ParamId::Glitch]
            .iter()
            .any(|id| pct(*id) > 0.0);
        let copy = if copy { 1.0 } else { 0.0 };
        if snap {
            self.creative_level.snap(level);
            self.copy.snap(copy);
        } else {
            self.creative_level.set(level);
            self.copy.set(copy);
        }
        let targets = [
            pct(ParamId::OctaveDown),
            pct(ParamId::Sub),
            pct(ParamId::OctaveUp),
        ];
        let follow = pct(ParamId::FormantFollow);
        self.freeze = on && v.get(ParamId::Freeze) >= 0.5;
        self.robot.amount = f64::from(pct(ParamId::Robot));
        self.glitch.amount = f64::from(pct(ParamId::Glitch));
        self.glitch.rate = GlitchRate::from_index(v.get(ParamId::GlitchRate));
        self.glitch.mode = GlitchMode::from_index(v.get(ParamId::GlitchMode));
        self.glitch.hold = on && v.get(ParamId::GlitchHold) >= 0.5;
        for (r, t) in self.layers.iter_mut().zip(targets) {
            if snap {
                r.snap(t);
            } else {
                r.set(t);
            }
        }
        if snap {
            self.follow.snap(follow);
        } else {
            self.follow.set(follow);
        }
    }

    /// Harmony voices from the parameters and the held notes, with the
    /// layer on.
    fn update_harmony(&mut self, snap: bool) {
        let on = self.values.get(ParamId::HarmonyOn) >= 0.5;
        let v = &self.values;
        let mode = if on {
            HarmonyMode::from_index(v.get(ParamId::Harmony))
        } else {
            HarmonyMode::Off
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let key = v.get(ParamId::Key).round().clamp(0.0, 11.0) as u8;
        let scale = match Scale::from_index(v.get(ParamId::Scale)) {
            // Thirds must be thirds: chromatic counts in the major key.
            Scale::Chromatic => Scale::Major,
            s => s,
        };
        let ids = [
            ParamId::Harmony1,
            ParamId::Harmony2,
            ParamId::Harmony3,
            ParamId::Harmony4,
        ];
        let mut intervals = [0; INTERVAL_VOICES];
        for (i, id) in intervals.iter_mut().zip(ids) {
            *i = interval_semitones(v.get(id));
        }
        let pct = |id: ParamId| v.get(id) / 100.0;
        let s = &mut self.harmony.settings;
        s.mode = mode;
        s.intervals = intervals;
        #[allow(clippy::cast_possible_truncation)]
        for (g, id) in s
            .voice_gain
            .iter_mut()
            .zip(crate::params::HARMONY_LEVEL_PARAMS)
        {
            *g = (v.get(id) / 100.0) as f32;
        }
        s.scale = scale_mask(key, scale);
        s.chord = crate::autotune::held_mask(&self.notes);
        s.expression = pct(ParamId::Expression);
        s.glide_s = v.get(ParamId::Glide) / 1000.0;
        s.humanize = pct(ParamId::HarmonyHumanize);
        s.character = pct(ParamId::Character);
        #[allow(clippy::cast_possible_truncation)]
        {
            s.spread = pct(ParamId::Spread) as f32;
        }
        s.vibrato = pct(ParamId::HarmonyVibrato);
        s.timing = pct(ParamId::HarmonyTiming);
        #[allow(clippy::cast_possible_truncation)]
        for (p, id) in s.pan.iter_mut().zip(crate::params::HARMONY_PAN_PARAMS) {
            *p = (v.get(id) / 100.0) as f32;
        }
        s.close = v.get(ParamId::CloseVoicing) >= 0.5;
        let double = if on { pct(ParamId::Double) } else { 0.0 };
        s.double = double > 0.0;
        // Latch: only where the notes play the harmonies.
        let latch = on && v.get(ParamId::Latch) >= 0.5;
        self.notes.set_latch(latch);
        self.harmony.apply_settings();
        self.harmony.set_notes(&self.notes);
        #[allow(clippy::cast_possible_truncation)]
        let double = double as f32;
        if snap {
            self.double.snap(double);
        } else {
            self.double.set(double);
        }
        #[allow(clippy::cast_possible_truncation)]
        // The lead's level is the lead layer's: it holds with every
        // layer.
        let lead = pct(ParamId::Lead) as f32;
        let level = if on {
            pct(ParamId::HarmonyLevel) as f32
        } else {
            0.0
        };
        if snap {
            self.lead.snap(lead);
            self.harmony_level.snap(level);
        } else {
            self.lead.set(lead);
            self.harmony_level.set(level);
        }
    }

    /// Solo: with any layer soloed, the others are silent.
    fn update_solo(&mut self, snap: bool) {
        let on = crate::params::SOLO_PARAMS.map(|id| self.values.get(id) >= 0.5);
        let any = on.iter().any(|s| *s);
        for (ramp, s) in self.solo.iter_mut().zip(on) {
            let g = if !any || s { 1.0 } else { 0.0 };
            if snap {
                ramp.snap(g);
            } else {
                ramp.set(g);
            }
        }
        let any = if any { 1.0 } else { 0.0 };
        if snap {
            self.soloing.snap(any);
        } else {
            self.soloing.set(any);
        }
    }

    /// Vocoder settings from the parameters and the held notes, with
    /// the layer on.
    fn update_vocoder(&mut self, snap: bool) {
        let on = self.values.get(ParamId::VocoderOn) >= 0.5;
        let v = &self.values;
        let pct = |id: ParamId| v.get(id) / 100.0;
        let mode = if on {
            VocoderMode::from_index(v.get(ParamId::Vocoder))
        } else {
            VocoderMode::Off
        };
        let s = &mut self.vocoder.settings;
        s.mode = mode;
        s.carrier = Carrier::from_index(v.get(ParamId::Carrier));
        s.bands = bands_from_index(v.get(ParamId::Bands));
        s.response_s = v.get(ParamId::Response) / 1000.0;
        s.shift_st = v.get(ParamId::BandShift);
        s.sibilance = pct(ParamId::Sibilance);
        #[allow(clippy::cast_possible_truncation)]
        {
            s.octave = v.get(ParamId::CarrierOctave).round() as i32 - 2;
        }
        s.detune_cents = v.get(ParamId::Detune);
        s.noise = pct(ParamId::CarrierNoise);
        s.release_s = v.get(ParamId::CarrierRelease) / 1000.0;
        s.follow = v.get(ParamId::CarrierFollow) >= 0.5;
        s.chord = FollowChord::from_index(v.get(ParamId::FollowChord));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let key = v.get(ParamId::Key).round().clamp(0.0, 11.0) as u8;
        s.scale = scale_mask(
            key,
            match Scale::from_index(v.get(ParamId::Scale)) {
                // Thirds must be thirds: chromatic counts in major.
                Scale::Chromatic => Scale::Major,
                sc => sc,
            },
        );
        s.glide_s = v.get(ParamId::CarrierGlide) / 1000.0;
        s.hold = v.get(ParamId::VocoderHold) >= 0.5;
        s.bend_range = v.get(ParamId::BendRange);
        s.width = pct(ParamId::VocoderWidth);
        s.lofi = pct(ParamId::VocoderLofi);
        s.consonants = pct(ParamId::Consonants);
        s.resonance = pct(ParamId::VocoderResonance);
        s.tilt_db = v.get(ParamId::VocoderTilt);
        s.gate_db = v.get(ParamId::VocoderGate);
        s.ensemble = pct(ParamId::Ensemble);
        s.sub = pct(ParamId::CarrierSub);
        s.whisper = pct(ParamId::Whisper);
        s.whisper_tone_db = v.get(ParamId::WhisperTone);
        // Inputs swapped: the voice comes by the side-chain, the track
        // (a synth) is the carrier.
        let swap = v.get(ParamId::VocoderSwap) >= 0.5;
        if swap {
            s.carrier = Carrier::Sidechain;
        }
        if swap != self.swap {
            self.swap = swap;
            self.detector.reset();
            self.shifter.reset();
        }
        self.vocoder.apply_settings();
        self.vocoder.set_notes(&self.notes);
        #[allow(clippy::cast_possible_truncation)]
        let level = if mode == VocoderMode::Off {
            0.0
        } else {
            pct(ParamId::VocoderLevel) as f32
        };
        if snap {
            self.vocoder_level.snap(level);
        } else {
            self.vocoder_level.set(level);
        }
    }

    /// The mix a Quackapella Link sends, for the key detection only
    /// (call before [`Self::process_with_sidechain`]).
    pub fn key_mix(&mut self, left: &[f32], right: &[f32]) {
        if self.key.mode() == KeyDetect::Off {
            return;
        }
        for (a, b) in left.iter().zip(right) {
            self.key.mix(0.5 * (sanitize(*a) + sanitize(*b)));
        }
    }

    /// What the key detection hears.
    #[must_use]
    pub fn key_reading(&self) -> KeyReading {
        self.key.reading()
    }

    /// A key found by Learn or Auto, to write to Key and Scale (once),
    /// with the scale in the family already chosen, and whether Learn is
    /// done (Key Detect goes back to Off).
    pub fn take_key(&mut self) -> Option<(u8, Scale, bool)> {
        let g = self.key.take_pending()?;
        let scale = g.scale_from(Scale::from_index(self.values.get(ParamId::Scale)));
        Some((g.tonic, scale, self.key.mode() == KeyDetect::Learn))
    }

    /// Notes the vocoder's carrier plays now (bit n = note n).
    #[must_use]
    pub fn vocoder_notes(&self) -> u128 {
        self.vocoder.sounding()
    }

    /// Notes the harmony voices sing now (bit n = note n).
    #[must_use]
    pub fn harmony_notes(&self) -> u128 {
        self.harmony.sounding()
    }

    /// Host tempo, bar length in beats and, while playing, the beat
    /// position at the start of the next block (the glitch steps follow
    /// them).
    pub fn set_transport(&mut self, tempo: f64, bar_beats: f64, beat: Option<f64>) {
        self.glitch.set_transport(tempo, bar_beats, beat);
        self.space.set_transport(tempo, bar_beats);
    }

    /// Whether a frozen grain is sounding (Creative layer).
    #[must_use]
    pub fn frozen(&self) -> bool {
        self.shifter.is_frozen()
    }

    /// Fill the gate's delay with its current state (after a reset, the
    /// gate does not fade in from closed).
    fn prime_gate(&mut self) {
        let g = self.gate.target();
        self.gate.snap(g);
        for _ in 0..self.gate_delay.delay() {
            self.gate_delay.tick(g);
        }
    }

    /// Autotune settings from the parameters, the mode and the held
    /// notes.
    fn update_tune(&mut self) {
        let v = &self.values;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let key = v.get(ParamId::Key).round().clamp(0.0, 11.0) as u8;
        let scale = Scale::from_index(v.get(ParamId::Scale));
        let source = Source::from_index(v.get(ParamId::Source));
        let enabled = NOTE_PARAMS.iter().enumerate().fold(0u16, |m, (i, p)| {
            if v.get(*p) >= 0.5 { m | (1 << i) } else { m }
        });
        let scale_notes = scale_mask(key, scale) & enabled;
        let held = crate::autotune::held_mask(&self.notes);
        let (mask, exact) = match source {
            Source::Scale => (scale_notes, None),
            _ if held == 0 => (scale_notes, None),
            Source::Midi => (held, None),
            Source::MidiExact => (held, self.notes.latest().map(|n| f64::from(n.note))),
        };
        let gated = v.get(ParamId::MidiGate) >= 0.5 && source != Source::Scale && held == 0;
        self.gate.set(if gated { 0.0 } else { 1.0 });
        let natural = self.mode == VoiceMode::Natural;
        let mut retune_s = v.get(ParamId::Retune) / 1000.0;
        let mut vibrato = v.get(ParamId::Vibrato) / 100.0;
        if natural {
            retune_s = retune_s.max(NATURAL_MIN_RETUNE_S);
            vibrato = vibrato.max(crate::autotune::NATURAL_MIN_VIBRATO);
        }
        self.retuner.settings = TuneSettings {
            amount: v.get(ParamId::Correction) / 100.0,
            retune_s,
            humanize: v.get(ParamId::Humanize) / 100.0,
            vibrato,
            mask,
            exact,
            natural,
            transition_s: v.get(ParamId::Transition) / 1000.0,
        };
    }

    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8) {
        self.notes.note_on(channel, note, velocity);
        self.update_tune();
        self.update_harmony(false);
        self.update_vocoder(false);
        self.robot.note = self.notes.latest().map(|n| f64::from(n.note));
    }

    pub fn note_off(&mut self, channel: u8, note: u8) {
        self.notes.note_off(channel, note);
        self.update_tune();
        self.update_harmony(false);
        self.update_vocoder(false);
        self.robot.note = self.notes.latest().map(|n| f64::from(n.note));
    }

    /// Sustain pedal (CC 64): released notes stay held while it is down.
    pub fn set_sustain(&mut self, down: bool) {
        self.notes.set_sustain(down);
        self.update_tune();
        self.update_harmony(false);
        self.update_vocoder(false);
        self.robot.note = self.notes.latest().map(|n| f64::from(n.note));
    }

    /// MIDI pitch bend, -1 to 1: bends the vocoder's carrier.
    pub fn pitch_bend(&mut self, bend: f64) {
        self.vocoder.set_bend(bend);
    }

    pub fn all_notes_off(&mut self) {
        self.notes.clear();
        self.update_tune();
        self.update_harmony(false);
        self.update_vocoder(false);
        self.robot.note = None;
    }

    /// Process a stereo block in place. A mono host passes the same
    /// signal on both sides and keeps the left one. Real-time safe.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.process_with_sidechain(left, right, None);
    }

    /// [`Self::process`] with the side-chain input (a carrier for the
    /// vocoder), mono or stereo. Real-time safe.
    pub fn process_with_sidechain(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        sidechain: Option<(&[f32], &[f32])>,
    ) {
        debug_assert_eq!(left.len(), right.len());
        let _ftz = crate::denormal::NoDenormals::new();
        #[allow(clippy::cast_possible_truncation)]
        let sr = self.sample_rate as f32;
        // A Link's mix fed this block takes the side-chain's place.
        let hear_side = !self.key.take_mix_fed()
            && sidechain.is_some()
            && !self.swap
            && self.vocoder.settings.carrier != Carrier::Sidechain;
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            // A NaN or an infinity from the host (or a broken plugin
            // before us) would stay in the filters and delay lines for
            // good: replaced by silence, absurd levels clamped.
            *l = sanitize(*l);
            *r = sanitize(*r);
            let (sc_l, sc_r) = sidechain.map_or((0.0, 0.0), |(a, b)| {
                (
                    sanitize(a.get(i).copied().unwrap_or(0.0)),
                    sanitize(b.get(i).copied().unwrap_or(0.0)),
                )
            });
            // Swapped inputs: the voice is the side-chain, the track is
            // the carrier and stays the dry signal.
            let (track_l, track_r) = (self.track[0].tick(*l), self.track[1].tick(*r));
            let (voice_l, voice_r, sc) = if self.swap {
                (sc_l, sc_r, 0.5 * (*l + *r))
            } else {
                (*l, *r, 0.5 * (sc_l + sc_r))
            };
            let g_in = self.in_gain.tick();
            let g_out = self.out_gain.tick();
            let mix = self.mix.tick();
            let byp = self.bypass.tick();
            let throat = self.throat.tick();
            let throat_link = self.throat_pitch.tick();
            #[allow(clippy::cast_possible_truncation)]
            let (transpose, formant_st) = if throat == 0.0 {
                (self.pitch.tick(), self.formant.tick())
            } else {
                let t = f64::from(throat);
                (
                    self.pitch.tick() + throat::pitch_st(t, f64::from(throat_link)) as f32,
                    self.formant.tick() + throat::formant_st(t) as f32,
                )
            };
            self.retuner.transpose = f64::from(transpose);
            self.retuner.transpose_first = self.keep_in_key;
            let formant = semitones_to_ratio(formant_st);
            let (in_l, in_r) = if self.fx_on {
                let (a, b) = self.tone.pre(voice_l * g_in, voice_r * g_in);
                let (a, b) = self.dynamics.tick(a, b);
                self.tone.post(a, b)
            } else {
                (voice_l * g_in, voice_r * g_in)
            };
            self.taps.input.push(0.5 * (voice_l + voice_r) * g_in);
            self.detector.push_sample(0.5 * (in_l + in_r));
            self.key.voice(self.detector.estimate());
            // The song's key from the mix too, when the side-chain
            // carries one (a Link's mix comes by `key_mix`).
            if hear_side {
                self.key.mix(0.5 * (sc_l + sc_r));
            }
            let period = self.detector.estimate().map(|e| sr / e.hz);
            let (dry_l, dry_r) = (self.dry[0].tick(voice_l), self.dry[1].tick(voice_r));
            // MIDI Gate: mutes the voice and the layers, not the
            // effects' tails (they ring on after the note).
            let gate = self.gate_delay.tick(self.gate.tick());
            self.shifter.set_layers([
                self.layers[0].tick(),
                self.layers[1].tick(),
                self.layers[2].tick(),
            ]);
            self.shifter.set_formant_follow(self.follow.tick());
            self.shifter.set_copy(self.copy.tick());
            let creative_level = self.creative_level.tick();
            // A held MIDI note plays the frozen grain (Pitch on top).
            #[allow(clippy::cast_possible_truncation)]
            let freeze_hz = self
                .robot
                .note
                .map(|n| (440.0 * ((n + f64::from(transpose) - 69.0) / 12.0).exp2()) as f32);
            self.shifter.set_freeze(self.freeze, freeze_hz, transpose);
            let lead = self.lead.tick();
            let level = self.harmony_level.tick();
            let double = self.double.tick();
            let voices = self.harmony.tick(level, double);
            self.shifter.set_voices(voices, lead, level);
            let mut steer = Steer {
                retuner: &mut self.retuner,
                robot: &mut self.robot,
                harmony: &mut self.harmony,
            };
            // The throat's body and top, before the grains (the formant
            // shift moves them with the rest of the tract).
            let (sh_l, sh_r) = self.throat_eq.tick(throat, in_l, in_r);
            let buses = self
                .shifter
                .tick_buses(sh_l, sh_r, period, &mut steer, formant);
            let voc_level = self.vocoder_level.tick();
            let vocoding = voc_level > 0.0 || self.vocoder.settings.mode != VocoderMode::Off;
            let follow = self
                .retuner
                .reading()
                .map(|t| t.out)
                .or_else(|| self.detector.estimate().map(|e| e.note()))
                .map(|n| f64::from(n + transpose));
            let tick4 = |r: &mut [Ramp; 4]| [r[0].tick(), r[1].tick(), r[2].tick(), r[3].tick()];
            let sends = tick4(&mut self.sends);
            let delays = tick4(&mut self.delay_sends);
            let choruses = tick4(&mut self.chorus_amounts);
            let drives = tick4(&mut self.drive_amounts);
            let fx_return = self.fx_return.tick();
            let solo = tick4(&mut self.solo);
            let soloing = self.soloing.tick();
            let fx = self.fx_on;
            let drive_on = fx && self.drive_on;
            // Each layer: its drive, then its share of the chorus, the
            // delay and the reverb.
            let mut bus = FxBus::default();
            let mut layer = |i: usize, x: (f32, f32), drv: &mut Distortion, solo: f32| {
                let g = gate * solo;
                let x = (x.0 * g, x.1 * g);
                let x = if drive_on {
                    drv.tick(x.0, x.1, drives[i])
                } else {
                    x
                };
                if fx {
                    bus.add(x, choruses[i], delays[i], sends[i]);
                } else {
                    bus.wet = (bus.wet.0 + x.0, bus.wet.1 + x.1);
                }
            };
            match self.routing {
                Routing::Parallel => {
                    // The glitch cuts the creative layer only.
                    let [cl, cr] = buses.creative;
                    let (cl, cr) = self.glitch.tick(cl, cr);
                    let (cl, cr) = (cl * creative_level, cr * creative_level);
                    layer(
                        0,
                        (buses.lead[0], buses.lead[1]),
                        &mut self.drives[0],
                        solo[0],
                    );
                    layer(1, (cl, cr), &mut self.drives[1], solo[1]);
                    layer(
                        2,
                        (buses.harmony[0], buses.harmony[1]),
                        &mut self.drives[2],
                        solo[2],
                    );
                    // Vocoder or talkbox, on top of the lead and harmonies.
                    if vocoding {
                        let dry = 0.5 * (dry_l + dry_r) * g_in;
                        let v = self.vocoder.tick(0.5 * (in_l + in_r), dry, sc, follow) * voc_level;
                        layer(3, self.vocoder.widen(v), &mut self.drives[3], solo[3]);
                    }
                }
                Routing::Series => {
                    // One output: the whole chain takes the voice's
                    // settings.
                    let s = self.series(
                        &buses,
                        creative_level,
                        voc_level,
                        vocoding,
                        sc,
                        follow,
                        solo,
                        soloing,
                    );
                    // The chain's solo is done inside it.
                    layer(0, s, &mut self.drives[0], 1.0);
                }
            }
            let (wet_l, wet_r) = if fx {
                self.chorus
                    .tick_send(bus.wet.0, bus.wet.1, bus.chorus.0, bus.chorus.1)
            } else {
                bus.wet
            };
            // What the layers made, for the editor's spectrum, then its
            // colour: EQ and exciter.
            self.taps.layers.push(0.5 * (wet_l + wet_r));
            let (wet_l, wet_r) = if fx {
                let (a, b) = self.eq.tick(wet_l, wet_r);
                self.exciter.tick(a, b)
            } else {
                (wet_l, wet_r)
            };
            let (wet_l, wet_r) = if self.space_on {
                // Ducked by the voice as it comes out (lined up with it).
                let key = 0.5 * (dry_l + dry_r) * g_in;
                let (fl, fr) = self.space.tick_sends(bus.delay, bus.reverb, key);
                if !self.fx_on && self.fx_return.is_settled() {
                    // Faded out: stop spending on the bus.
                    self.space_on = false;
                }
                (wet_l + fl * fx_return, wet_r + fr * fx_return)
            } else {
                (wet_l, wet_r)
            };
            // Output dynamics on the whole processed sound.
            let (wet_l, wet_r) = if fx {
                self.output.tick(wet_l, wet_r)
            } else {
                (wet_l, wet_r)
            };
            self.taps.output.push(0.5 * (wet_l + wet_r));
            self.taps.side.push(0.5 * (sc_l + sc_r));
            let (dry_l, dry_r) = if self.swap {
                (track_l, track_r)
            } else {
                (dry_l, dry_r)
            };
            let (dry_l, dry_r) = (dry_l * gate, dry_r * gate);
            let out_l = (dry_l + (wet_l - dry_l) * mix) * g_out;
            let out_r = (dry_r + (wet_r - dry_r) * mix) * g_out;
            *l = out_l + (dry_l - out_l) * byp;
            *r = out_r + (dry_r - out_r) * byp;
        }
    }

    /// One sample of the series chain (see [`crate::chain`]): the
    /// voice and Creative's pitch layers at the source, then each module
    /// on what the previous ones made.
    #[allow(clippy::too_many_arguments)]
    fn series(
        &mut self,
        buses: &Buses,
        creative_level: f32,
        voc_level: f32,
        vocoding: bool,
        sidechain: f32,
        follow: Option<f64>,
        solo: [f32; 4],
        soloing: f32,
    ) -> (f32, f32) {
        let [cl, cr] = buses.creative;
        let mut s = (
            buses.lead[0] + cl * creative_level,
            buses.lead[1] + cr * creative_level,
        );
        // Solo: the soloed layers alone, as they go into the chain.
        let mut alone = (
            buses.lead[0] * solo[0] + cl * creative_level * solo[1],
            buses.lead[1] * solo[0] + cr * creative_level * solo[1],
        );
        // Harmonies after the vocoder: its internal carrier sings them
        // (the vocoder's pitch is its carrier's), and what passes by the
        // vocoder gets them as they are.
        let carried = vocoding
            && self.vocoder.settings.carrier != Carrier::Sidechain
            && self.chain.before(Stage::Vocoder, Stage::Harmony);
        self.vocoder.set_extra_notes(if carried {
            self.harmony.sung_notes()
        } else {
            [None; crate::vocoder::EXTRA_NOTES]
        });
        let mut pass = 1.0;
        for stage in self.chain.0 {
            match stage {
                Stage::Creative => s = self.glitch.tick(s.0, s.1),
                Stage::Harmony => {
                    let g = if carried { pass } else { 1.0 };
                    s = (s.0 + buses.harmony[0] * g, s.1 + buses.harmony[1] * g);
                    alone = (
                        alone.0 + buses.harmony[0] * solo[2],
                        alone.1 + buses.harmony[1] * solo[2],
                    );
                }
                Stage::Vocoder if vocoding => {
                    let m = 0.5 * (s.0 + s.1);
                    let v = self.vocoder.tick(m, m, sidechain, follow);
                    let (vl, vr) = self.vocoder.widen(v);
                    let (d, w) = crossfade(voc_level);
                    pass = d;
                    s = (s.0 * d + vl * w, s.1 * d + vr * w);
                    alone = (alone.0 + vl * w * solo[3], alone.1 + vr * w * solo[3]);
                }
                Stage::Vocoder => {}
            }
        }
        (
            s.0 + (alone.0 - s.0) * soloing,
            s.1 + (alone.1 - s.1) * soloing,
        )
    }
}

/// The layers' sum and what they send to the chorus, delay and reverb.
#[derive(Debug, Clone, Copy, Default)]
struct FxBus {
    wet: (f32, f32),
    chorus: (f32, f32),
    delay: (f32, f32),
    reverb: (f32, f32),
}

impl FxBus {
    #[inline]
    fn add(&mut self, x: (f32, f32), chorus: f32, delay: f32, reverb: f32) {
        self.wet = (self.wet.0 + x.0, self.wet.1 + x.1);
        self.chorus = (self.chorus.0 + x.0 * chorus, self.chorus.1 + x.1 * chorus);
        self.delay = (self.delay.0 + x.0 * delay, self.delay.1 + x.1 * delay);
        self.reverb = (self.reverb.0 + x.0 * reverb, self.reverb.1 + x.1 * reverb);
    }
}

/// Highest input level taken as is (+30 dBFS): only garbage is above.
const INPUT_CLAMP: f32 = 32.0;

/// Input sample made safe for the filters: NaN and infinities become
/// silence, levels beyond [`INPUT_CLAMP`] are clamped. Free for normal
/// audio (it comes out bit for bit).
#[inline]
fn sanitize(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-INPUT_CLAMP, INPUT_CLAMP)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impulse_response(e: &mut Engine, n: usize) -> Vec<f32> {
        let mut l = vec![0.0; n];
        let mut r = vec![0.0; n];
        l[0] = 1.0;
        r[0] = 1.0;
        e.process(&mut l, &mut r);
        assert_eq!(l, r);
        l
    }

    #[test]
    fn effects_add_no_latency() {
        // FX on, dynamics neutral: the direct sound still lands on the
        // reported latency, the delay and reverb come after it.
        let mut e = Engine::new();
        e.prepare(48_000.0, 512);
        e.set_param(ParamId::FxOn, 1.0);
        e.set_param(ParamId::DelayLevel, 50.0);
        e.set_param(ParamId::VoiceReverb, 100.0);
        e.reset();
        let lat = e.latency_samples();
        // Reverb tails differ left and right: the left side only.
        let mut y = vec![0.0; 48_000];
        let mut r = vec![0.0; 48_000];
        y[0] = 0.5;
        r[0] = 0.5;
        e.process(&mut y, &mut r);
        assert!((y[lat] - 0.5).abs() < 1e-3, "direct {}", y[lat]);
        assert!(y[..lat].iter().all(|v| v.abs() < 1e-6));
        let tail: f32 = y[lat + 1..].iter().map(|v| v.abs()).sum();
        assert!(tail > 0.01, "no tail {tail}");
    }

    #[test]
    fn output_is_the_input_delayed_by_the_reported_latency() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let mut e = Engine::new();
            e.prepare(sr, 512);
            let lat = e.latency_samples();
            assert!(lat > 0);
            let y = impulse_response(&mut e, lat + 64);
            for (i, v) in y.iter().enumerate() {
                let want = if i == lat { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-6, "sr {sr}, sample {i}: {v}");
            }
        }
    }

    #[test]
    fn latency_does_not_depend_on_mode_mix_or_bypass() {
        let mut e = Engine::new();
        let lat = e.latency_samples();
        let mut v = ParamValues::default();
        v.set(ParamId::Mode, 1.0);
        v.set(ParamId::CreativeOn, 1.0);
        v.set(ParamId::Mix, 40.0);
        v.set(ParamId::InputGain, 6.0);
        e.set_all(&v, true);
        let y = impulse_response(&mut e, lat + 8);
        let peak = y
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()));
        assert_eq!(peak.map(|p| p.0), Some(lat));
        v.set(ParamId::Bypass, 1.0);
        e.reset();
        e.set_all(&v, true);
        let y = impulse_response(&mut e, lat + 8);
        assert!((y[lat] - 1.0).abs() < 1e-6);
        assert_eq!(e.mode(), VoiceMode::Tune);
    }

    #[test]
    fn gains_and_mix_apply_after_smoothing() {
        let mut e = Engine::new();
        let mut v = ParamValues::default();
        v.set(ParamId::OutputGain, -6.0);
        v.set(ParamId::Mix, 50.0);
        v.set(ParamId::InputGain, 6.0);
        e.set_all(&v, true);
        let n = e.latency_samples() + 256;
        let mut l = vec![0.5; n];
        let mut r = vec![0.5; n];
        e.process(&mut l, &mut r);
        let want = (0.5 + (0.5 * db_to_gain(6.0) - 0.5) * 0.5) * db_to_gain(-6.0);
        assert!((l[n - 1] - want).abs() < 1e-5, "{} vs {want}", l[n - 1]);
    }

    #[test]
    fn parameter_changes_ramp_without_jumps() {
        let mut e = Engine::new();
        let n = e.latency_samples() + 4096;
        let mut l = vec![0.5; n];
        let mut r = vec![0.5; n];
        e.process(&mut l, &mut r);
        e.set_param(ParamId::OutputGain, -24.0);
        e.set_param(ParamId::Bypass, 1.0);
        e.set_param(ParamId::Bypass, 0.0);
        let mut l = vec![0.5; 4096];
        let mut r = vec![0.5; 4096];
        e.process(&mut l, &mut r);
        let max_step = l
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(max_step < 1e-3, "step {max_step}");
    }

    #[test]
    fn detects_the_input_pitch() {
        let mut e = Engine::new();
        #[allow(clippy::cast_precision_loss)]
        let x: Vec<f32> = (0..9_600)
            .map(|i| 0.3 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin())
            .collect();
        let (mut l, mut r) = (x.clone(), x);
        e.process(&mut l, &mut r);
        let p = e.pitch().expect("voiced");
        assert!((p.note() - 57.0).abs() < 0.05, "{}", p.hz);
        e.reset();
        assert!(e.pitch().is_none());
    }

    #[test]
    fn transposes_the_voice() {
        let mut e = Engine::new();
        let mut v = ParamValues::default();
        v.set(ParamId::Pitch, 7.0);
        v.set(ParamId::Fine, -20.0);
        e.set_all(&v, true);
        #[allow(clippy::cast_precision_loss)]
        let x: Vec<f32> = (0..24_000)
            .map(|i| 0.3 * (std::f32::consts::TAU * 196.0 * i as f32 / 48_000.0).sin())
            .collect();
        let (mut l, mut r) = (x.clone(), x);
        e.process(&mut l, &mut r);
        assert_eq!(l, r);
        let mut det = Yin::new(48_000.0);
        det.push(&l[12_000..]);
        let got = det.estimate().expect("voiced").note();
        let want = crate::pitch::hz_to_midi(196.0) + 6.8;
        assert!((got - want).abs() < 0.1, "{got} vs {want}");
    }

    /// A sine around A3 sung `cents` off through the engine;
    /// median output note.
    fn tuned_note(v: &ParamValues, cents: f64, midi: Option<u8>) -> f32 {
        let mut e = Engine::new();
        e.set_all(v, true);
        if let Some(n) = midi {
            e.note_on(0, n, 100);
        }
        let hz = 440.0 * ((57.0 + cents / 100.0 - 69.0) / 12.0f64).exp2();
        let mut phase = 0.0;
        #[allow(clippy::cast_possible_truncation)]
        let x: Vec<f32> = (0..48_000)
            .map(|_| {
                phase += hz / 48_000.0;
                phase -= phase.floor();
                (0.3 * (std::f64::consts::TAU * phase).sin()) as f32
            })
            .collect();
        let (mut l, mut r) = (x.clone(), x);
        e.process(&mut l, &mut r);
        let mut det = Yin::new(48_000.0);
        let mut notes = Vec::new();
        for c in l[12_000..].chunks(240) {
            det.push(c);
            if let Some(p) = det.estimate() {
                notes.push(p.note());
            }
        }
        assert!(notes.len() > 100);
        notes.sort_by(f32::total_cmp);
        notes[notes.len() / 2]
    }

    #[test]
    fn hard_tune_lands_on_the_allowed_notes() {
        let mut v = ParamValues::default();
        v.set(ParamId::Mode, 1.0);
        v.set(ParamId::Retune, 0.0);
        v.set(ParamId::Vibrato, 0.0);
        v.set(ParamId::Humanize, 0.0);
        let got = tuned_note(&v, 38.0, None);
        assert!((got - 57.0).abs() < 0.03, "{got}");
        // A switched off: the voice goes to A# (nearer than G#).
        v.set(ParamId::NoteA, 0.0);
        let got = tuned_note(&v, 38.0, None);
        assert!((got - 58.0).abs() < 0.03, "{got}");
    }

    #[test]
    fn midi_exact_sings_the_played_note() {
        let mut v = ParamValues::default();
        v.set(ParamId::Mode, 1.0);
        v.set(ParamId::Source, 2.0);
        v.set(ParamId::Retune, 10.0);
        let got = tuned_note(&v, -20.0, Some(64));
        assert!((got - 64.0).abs() < 0.05, "{got}");
    }

    #[test]
    fn correction_off_leaves_the_pitch_alone() {
        let mut v = ParamValues::default();
        v.set(ParamId::Correction, 0.0);
        let got = tuned_note(&v, 38.0, None);
        assert!((got - 57.38).abs() < 0.05, "{got}");
    }

    #[test]
    fn midi_gate_mutes_without_a_note() {
        let mut e = Engine::new();
        let mut v = ParamValues::default();
        v.set(ParamId::Source, 1.0);
        v.set(ParamId::MidiGate, 1.0);
        e.set_all(&v, true);
        e.reset();
        let n = 4_800;
        let run = |e: &mut Engine| {
            let mut l = vec![0.5; n];
            let mut r = vec![0.5; n];
            e.process(&mut l, &mut r);
            l[n - 1]
        };
        assert!(run(&mut e).abs() < 1e-6);
        e.note_on(0, 60, 100);
        assert!((run(&mut e) - 0.5).abs() < 1e-3);
        e.note_off(0, 60);
        assert!(run(&mut e).abs() < 1e-6);
        // Scale source: the gate does not apply.
        e.set_param(ParamId::Source, 0.0);
        assert!((run(&mut e) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn midi_gate_leaves_the_effect_tails() {
        let mut e = Engine::new();
        let mut v = ParamValues::default();
        v.set(ParamId::Source, 1.0);
        v.set(ParamId::MidiGate, 1.0);
        v.set(ParamId::FxOn, 1.0);
        v.set(ParamId::ReverbLevel, 100.0);
        v.set(ParamId::VoiceReverb, 100.0);
        v.set(ParamId::ReverbDecay, 4.0);
        e.set_all(&v, true);
        e.reset();
        let n = 4_800;
        let sine = |k: usize| {
            (0..n)
                .map(|i| {
                    #[allow(clippy::cast_precision_loss)]
                    let t = (k * n + i) as f32 / 48_000.0;
                    0.3 * (std::f32::consts::TAU * 220.0 * t).sin()
                })
                .collect::<Vec<f32>>()
        };
        e.note_on(0, 57, 100);
        for k in 0..5 {
            let mut l = sine(k);
            let mut r = l.clone();
            e.process(&mut l, &mut r);
        }
        e.note_off(0, 57);
        let mut l = sine(5);
        let mut r = l.clone();
        e.process(&mut l, &mut r);
        // Gate closed: the reverb still rings.
        let tail: f32 = l[n / 2..].iter().map(|x| x * x).sum::<f32>() / (n / 2) as f32;
        assert!(tail.sqrt() > 1e-3, "{}", tail.sqrt());
    }

    #[test]
    fn creative_effects_need_their_layer() {
        let x: Vec<f32> = (0..24_000)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = i as f32 / 48_000.0;
                0.3 * (std::f32::consts::TAU * 220.0 * t).sin()
                    + 0.1 * (std::f32::consts::TAU * 440.0 * t).sin()
            })
            .collect();
        let run = |mode: f64, creative: bool| {
            let mut e = Engine::new();
            let mut v = ParamValues::default();
            v.set(ParamId::Mode, mode);
            if creative {
                for id in [
                    ParamId::FormantFollow,
                    ParamId::OctaveDown,
                    ParamId::Sub,
                    ParamId::OctaveUp,
                    ParamId::Robot,
                    ParamId::Glitch,
                ] {
                    v.set(id, 100.0);
                }
                v.set(ParamId::Freeze, 1.0);
                v.set(ParamId::Pitch, 5.0);
            }
            e.set_all(&v, true);
            let mut l = x.clone();
            let mut r = x.clone();
            e.process(&mut l, &mut r);
            l
        };
        for mode in [0.0, 1.0] {
            let plain = run(mode, false);
            let mut e = Engine::new();
            let mut v = ParamValues::default();
            v.set(ParamId::Mode, mode);
            v.set(ParamId::Pitch, 5.0);
            e.set_all(&v, true);
            let mut l = x.clone();
            let mut r = x.clone();
            e.process(&mut l, &mut r);
            let with = run(mode, true);
            assert_eq!(l, with, "mode {mode}");
            assert_ne!(plain, with);
        }
        // Layer on: the effects play over either style.
        let mut e = Engine::new();
        let mut v = ParamValues::default();
        v.set(ParamId::Pitch, 5.0);
        v.set(ParamId::CreativeOn, 1.0);
        v.set(ParamId::OctaveDown, 100.0);
        e.set_all(&v, true);
        let mut l = x.clone();
        let mut r = x.clone();
        e.process(&mut l, &mut r);
        let mut e = Engine::new();
        v.set(ParamId::CreativeOn, 0.0);
        e.set_all(&v, true);
        let (mut l0, mut r0) = (x.clone(), x.clone());
        e.process(&mut l0, &mut r0);
        assert_ne!(l, l0);
    }

    /// Harmony layer on, the lead muted, one harmony voice `semis`
    /// away.
    fn harmony_only(mode: f64, semis: i32) -> ParamValues {
        let mut v = ParamValues::default();
        v.set(ParamId::HarmonyOn, 1.0);
        v.set(ParamId::Harmony, mode);
        v.set(ParamId::Lead, 0.0);
        v.set(ParamId::HarmonyLevel, 100.0);
        v.set(ParamId::Harmony1, crate::harmony::interval_index(semis));
        v.set(ParamId::Harmony2, crate::harmony::interval_index(0));
        v.set(ParamId::HarmonyHumanize, 0.0);
        v
    }

    #[test]
    fn harmony_sings_a_third_in_the_key() {
        // A3 in C major: a third above is C4, a sixth below C3.
        for semis in [3, 4] {
            let got = tuned_note(&harmony_only(2.0, semis), 0.0, None);
            assert!((got - 60.0).abs() < 0.05, "{got}");
        }
        let got = tuned_note(&harmony_only(2.0, -9), 0.0, None);
        assert!((got - 48.0).abs() < 0.05, "{got}");
        // Fixed: the exact interval, out of the key.
        let got = tuned_note(&harmony_only(1.0, 4), 0.0, None);
        assert!((got - 61.0).abs() < 0.05, "{got}");
        // A minor (key A, minor): a third above A is C too, a second B.
        let mut v = harmony_only(2.0, 2);
        v.set(ParamId::Key, 9.0);
        v.set(ParamId::Scale, 2.0);
        let got = tuned_note(&v, 0.0, None);
        assert!((got - 59.0).abs() < 0.05, "{got}");
    }

    #[test]
    fn harmony_plays_the_midi_note() {
        let got = tuned_note(&harmony_only(0.0, 0), 0.0, Some(64));
        assert!((got - 64.0).abs() < 0.05, "{got}");
    }

    #[test]
    fn harmony_adds_no_latency_and_needs_its_layer() {
        let mut e = Engine::new();
        let lat = e.latency_samples();
        let mut v = harmony_only(2.0, 4);
        v.set(ParamId::Lead, 100.0);
        e.set_all(&v, true);
        assert_eq!(e.latency_samples(), lat);
        // Impulse (unvoiced): only the lead, on time.
        let y = impulse_response(&mut e, lat + 8);
        assert!((y[lat] - 1.0).abs() < 1e-6);
        // Layer off: no harmony at all, even with the knobs set.
        v.set(ParamId::HarmonyOn, 0.0);
        let mut e = Engine::new();
        e.set_all(&v, true);
        assert_eq!(e.harmony.settings.mode, HarmonyMode::Off);
    }

    /// A sung A3 (220 Hz), half a second.
    fn vowel() -> Vec<f32> {
        #[allow(clippy::cast_precision_loss)]
        (0..24_000)
            .map(|i| 0.3 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin())
            .collect()
    }

    #[test]
    fn series_without_modules_is_the_parallel_voice() {
        let x = vowel();
        let mut out = Vec::new();
        for routing in [0.0, 1.0] {
            let mut v = ParamValues::default();
            v.set(ParamId::Routing, routing);
            v.set(ParamId::Chain, 3.0);
            let mut e = Engine::new();
            e.set_all(&v, true);
            let (mut l, mut r) = (x.clone(), x.clone());
            e.process(&mut l, &mut r);
            out.push(l);
        }
        assert_eq!(out[0], out[1]);
    }

    #[test]
    fn series_keeps_the_latency_in_every_order() {
        let mut e = Engine::new();
        let lat = e.latency_samples();
        for chain in 0..Chain::COUNT {
            let mut v = harmony_only(2.0, 4);
            v.set(ParamId::Lead, 100.0);
            v.set(ParamId::VocoderOn, 1.0);
            v.set(ParamId::CreativeOn, 1.0);
            v.set(ParamId::Routing, 1.0);
            v.set(ParamId::Chain, f64::from(chain));
            e.set_all(&v, true);
            assert_eq!(e.latency_samples(), lat);
            let (mut l, mut r) = (vowel(), vowel());
            e.process(&mut l, &mut r);
            assert!(l.iter().all(|s| s.is_finite() && s.abs() < 4.0));
        }
    }

    #[test]
    fn harmony_after_the_vocoder_plays_on_its_carrier() {
        // A major third over the voice, the carrier following the voice.
        let run = |chain: f64| {
            let mut v = harmony_only(1.0, 4);
            v.set(ParamId::Lead, 100.0);
            v.set(ParamId::VocoderOn, 1.0);
            v.set(ParamId::Vocoder, 0.0);
            v.set(ParamId::CarrierFollow, 1.0);
            v.set(ParamId::Routing, 1.0);
            v.set(ParamId::Chain, chain);
            let mut e = Engine::new();
            e.set_all(&v, true);
            let (mut l, mut r) = (vowel(), vowel());
            e.process(&mut l, &mut r);
            e.vocoder_notes()
        };
        // Creative > Harmony > Vocoder: the carrier sings the voice only.
        let before = run(0.0);
        assert_ne!(before & (1 << 57), 0);
        assert_eq!(before & (1 << 61), 0);
        // Creative > Vocoder > Harmony: and the third (C#4).
        let after = run(2.0);
        assert_ne!(after & (1 << 57), 0);
        assert_ne!(after & (1 << 61), 0);
    }

    #[test]
    fn vocoder_adds_no_latency_and_needs_its_layer() {
        let mut v = ParamValues::default();
        v.set(ParamId::VocoderOn, 1.0);
        v.set(ParamId::Vocoder, 0.0);
        let mut e = Engine::new();
        let lat = e.latency_samples();
        e.set_all(&v, true);
        assert_eq!(e.latency_samples(), lat);
        e.note_on(0, 48, 100);
        // A sung vowel: the carrier sounds under it.
        #[allow(clippy::cast_precision_loss)]
        let x: Vec<f32> = (0..24_000)
            .map(|i| 0.3 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin())
            .collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        e.process(&mut l, &mut r);
        assert!(l.iter().all(|s| s.is_finite()));
        assert_ne!(e.vocoder_notes() & (1 << 48), 0);
        // Layer off: no vocoder.
        v.set(ParamId::VocoderOn, 0.0);
        e.set_all(&v, true);
        assert_eq!(e.vocoder.settings.mode, VocoderMode::Off);
    }

    #[test]
    fn every_layer_plays_together() {
        let mut v = ParamValues::default();
        v.set(ParamId::Mode, 1.0);
        for id in [ParamId::CreativeOn, ParamId::HarmonyOn, ParamId::VocoderOn] {
            v.set(id, 1.0);
        }
        v.set(ParamId::OctaveDown, 60.0);
        v.set(ParamId::Robot, 50.0);
        let mut e = Engine::new();
        let lat = e.latency_samples();
        e.set_all(&v, true);
        assert_eq!(e.latency_samples(), lat);
        e.note_on(0, 48, 100);
        assert_ne!(e.harmony.settings.mode, HarmonyMode::Off);
        assert_ne!(e.vocoder.settings.mode, VocoderMode::Off);
        #[allow(clippy::cast_precision_loss)]
        let x: Vec<f32> = (0..24_000)
            .map(|i| 0.3 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin())
            .collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        e.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|s| s.is_finite() && s.abs() < 4.0));
        assert_ne!(e.harmony_notes(), 0);
        assert_ne!(e.vocoder_notes() & (1 << 48), 0);
    }

    /// RMS of the engine's left output over the second half of `x`.
    fn rms_of(v: &ParamValues, x: &[f32]) -> f32 {
        let mut e = Engine::new();
        e.set_all(v, true);
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        e.process(&mut l, &mut r);
        let half = &l[l.len() / 2..];
        #[allow(clippy::cast_precision_loss)]
        let n = half.len() as f32;
        (half.iter().map(|s| s * s).sum::<f32>() / n).sqrt()
    }

    #[test]
    fn each_layer_is_heard_alone() {
        #[allow(clippy::cast_precision_loss)]
        let vowel: Vec<f32> = (0..24_000)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                0.3 * (std::f32::consts::TAU * 220.0 * t).sin()
                    + 0.1 * (std::f32::consts::TAU * 440.0 * t).sin()
            })
            .collect();
        let mut seed: u32 = 1;
        let noise: Vec<f32> = (0..24_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                #[allow(clippy::cast_precision_loss)]
                let u = (seed >> 8) as f32 / (1u32 << 24) as f32;
                0.2 * (2.0 * u - 1.0)
            })
            .collect();
        let mut v = ParamValues::default();
        v.set(ParamId::Lead, 0.0);
        assert!(rms_of(&v, &vowel) < 1e-4, "lead down: silence");
        // Creative alone: its copy of the lead (robot) is heard.
        let mut c = v;
        c.set(ParamId::CreativeOn, 1.0);
        c.set(ParamId::Robot, 100.0);
        let got = rms_of(&c, &vowel);
        assert!(got > 0.1, "creative alone: {got}");
        // The creative level scales it.
        c.set(ParamId::CreativeLevel, 50.0);
        let half = rms_of(&c, &vowel);
        assert!((half / got - 0.5).abs() < 0.05, "{half} vs {got}");
        // Harmonies alone keep the consonants (noise is never sung).
        let mut h = v;
        h.set(ParamId::HarmonyOn, 1.0);
        h.set(ParamId::Harmony, 1.0);
        let got = rms_of(&h, &noise);
        assert!(got > 0.05, "consonants with the harmonies: {got}");
        // A voice's own level.
        let mut h = v;
        h.set(ParamId::HarmonyOn, 1.0);
        h.set(ParamId::Harmony, 1.0);
        h.set(ParamId::Harmony2, crate::harmony::interval_index(0));
        let full = rms_of(&h, &vowel);
        h.set(ParamId::HarmonyLevel1, 0.0);
        let muted = rms_of(&h, &vowel);
        assert!(full > 0.05 && muted < 0.1 * full, "{full} {muted}");
    }

    #[test]
    fn notes_follow_midi() {
        let mut e = Engine::new();
        e.note_on(0, 60, 100);
        e.note_on(0, 67, 100);
        e.note_off(0, 60);
        assert_eq!(e.notes().latest().map(|n| n.note), Some(67));
        e.all_notes_off();
        assert!(e.notes().held().is_empty());
    }

    #[test]
    fn swapped_inputs_vocode_the_side_chain_voice_on_the_track() {
        let mut e = Engine::new();
        let lat = e.latency_samples();
        let mut v = ParamValues::default();
        v.set(ParamId::VocoderOn, 1.0);
        v.set(ParamId::VocoderSwap, 1.0);
        v.set(ParamId::Lead, 0.0);
        v.set(ParamId::VocoderLevel, 100.0);
        e.set_all(&v, true);
        let n = 48_000;
        // Track: a 110 Hz saw (the synth); side-chain: a buzzy voice.
        let synth: Vec<f32> = (0..n)
            .map(|i| (2.0 * ((i as f64 * 110.0 / 48_000.0) % 1.0) - 1.0) as f32 * 0.3)
            .collect();
        let voice: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                (0.3 * (std::f64::consts::TAU * 220.0 * t).sin()
                    + 0.2 * (std::f64::consts::TAU * 660.0 * t).sin()) as f32
            })
            .collect();
        let rms = |x: &[f32]| {
            (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
        };
        let (mut l, mut r) = (synth.clone(), synth.clone());
        e.process_with_sidechain(&mut l, &mut r, Some((&voice, &voice)));
        assert!(rms(&l[n / 2..]) > 0.01, "vocoded: {}", rms(&l[n / 2..]));
        // No voice in the side-chain: the synth is not vocoded.
        let (mut l, mut r) = (synth.clone(), synth.clone());
        let silent = vec![0.0; n];
        e.process_with_sidechain(&mut l, &mut r, Some((&silent, &silent)));
        assert!(rms(&l[n / 2..]) < 1e-3, "silent: {}", rms(&l[n / 2..]));
        // Dry and bypass are the track, lined up with the latency.
        v.set(ParamId::Bypass, 1.0);
        e.set_all(&v, true);
        let (mut l, mut r) = (synth.clone(), synth.clone());
        e.process_with_sidechain(&mut l, &mut r, Some((&voice, &voice)));
        for i in n / 2..n / 2 + 100 {
            assert!((l[i] - synth[i - lat]).abs() < 1e-5);
        }
    }

    #[test]
    fn solo_mutes_the_other_layers() {
        // The lead alone, then soloing the (silent) harmony layer: the
        // lead must go quiet, in parallel and in series.
        for routing in [0.0, 1.0] {
            let render = |solo_harmony: bool| {
                let mut e = Engine::default();
                e.prepare(48_000.0, 512);
                let mut v = ParamValues::default();
                v.set(ParamId::Routing, routing);
                v.set(ParamId::Mix, 100.0);
                v.set(ParamId::SoloHarmony, if solo_harmony { 1.0 } else { 0.0 });
                e.set_all(&v, true);
                let mut peak = 0.0_f32;
                let mut n = 0u32;
                for _ in 0..40 {
                    let mut l = [0.0_f32; 512];
                    for x in &mut l {
                        #[allow(clippy::cast_precision_loss)]
                        let t = n as f32 / 48_000.0;
                        *x = 0.4 * (std::f32::consts::TAU * 220.0 * t).sin();
                        n += 1;
                    }
                    let mut r = l;
                    e.process(&mut l, &mut r);
                    peak = l.iter().fold(peak, |a, b| a.max(b.abs()));
                }
                peak
            };
            assert!(render(false) > 0.05, "routing {routing}");
            assert!(render(true) < 1e-3, "routing {routing}");
        }
    }
}
