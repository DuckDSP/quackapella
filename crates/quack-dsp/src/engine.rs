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
//! The shifter delays the voice by exactly the fixed latency
//! ([`crate::latency::LATENCY_MS`]), like the dry delay lines, so
//! dry/wet and bypass never comb and the host's compensation is always
//! right.
//!
//! The autotune ([`Retuner`]) steers the shifter grain by grain: it
//! gets each grain's own period and returns the grain's pitch ratio
//! (correction plus transposition). Harmonies (next steps) will add
//! voices on the same marks.

use crate::autotune::{Retuner, Scale, Source, TuneReading, TuneSettings, scale_mask};
use crate::db_to_gain;
use crate::latency::{DelayLine, latency_samples};
use crate::midi::NoteStack;
use crate::mode::VoiceMode;
use crate::params::{NOTE_PARAMS, ParamId, ParamValues};
use crate::pitch::{PitchDetector, PitchEstimate, Yin};
use crate::shift::{Psola, semitones_to_ratio};

/// Fastest retune in the Natural mode: below it the correction is
/// heard as an effect, which is the Tune mode's job.
pub const NATURAL_MIN_RETUNE_S: f64 = 0.03;
use crate::smooth::Ramp;

/// Smoothing time for gains and mix.
const SMOOTH_MS: f64 = 20.0;
/// Bypass crossfade.
const BYPASS_MS: f64 = 10.0;

pub const CHANNELS: usize = 2;

#[derive(Debug, Clone)]
pub struct Engine {
    sample_rate: f64,
    latency: usize,
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
    dry: [DelayLine; CHANNELS],
    notes: NoteStack,
    detector: Yin,
    shifter: Psola,
    retuner: Retuner,
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
            values: ParamValues::default(),
            mode: VoiceMode::default(),
            in_gain: Ramp::new(1.0),
            out_gain: Ramp::new(1.0),
            mix: Ramp::new(1.0),
            bypass: Ramp::new(0.0),
            pitch: Ramp::new(0.0),
            formant: Ramp::new(0.0),
            dry: Default::default(),
            notes: NoteStack::default(),
            detector: Yin::default(),
            shifter: Psola::default(),
            retuner: Retuner::default(),
        };
        e.prepare(48_000.0, 512);
        e
    }

    /// Allocates delay lines and resets state. Not for the audio thread.
    pub fn prepare(&mut self, sample_rate: f64, max_block: usize) {
        self.sample_rate = sample_rate;
        self.latency = latency_samples(sample_rate);
        for d in &mut self.dry {
            d.prepare(self.latency);
        }
        for r in [
            &mut self.in_gain,
            &mut self.out_gain,
            &mut self.mix,
            &mut self.pitch,
            &mut self.formant,
        ] {
            r.prepare(sample_rate, SMOOTH_MS);
        }
        self.bypass.prepare(sample_rate, BYPASS_MS);
        self.detector.prepare(sample_rate, max_block);
        self.shifter.prepare(sample_rate, self.latency);
        self.retuner.prepare(sample_rate);
        self.notes.clear();
        self.apply_all(true);
    }

    /// Clears audio state (transport jump, host reset). Real-time safe.
    pub fn reset(&mut self) {
        for d in &mut self.dry {
            d.reset();
        }
        self.detector.reset();
        self.shifter.reset();
        self.retuner.reset();
        self.notes.clear();
        self.apply_all(true);
    }

    /// The latency the host must compensate, in samples. Constant for a
    /// given sample rate, whatever the mode or parameters.
    #[must_use]
    pub fn latency_samples(&self) -> usize {
        self.latency
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
            ParamId::Mode => {
                self.mode = VoiceMode::from_index(f64::from(v));
                self.update_tune();
                return;
            }
            _ => {
                self.update_tune();
                return;
            }
        };
        if snap {
            ramp.snap(target);
        } else {
            ramp.set(target);
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
        let mut retune_s = v.get(ParamId::Retune) / 1000.0;
        if self.mode == VoiceMode::Natural {
            retune_s = retune_s.max(NATURAL_MIN_RETUNE_S);
        }
        self.retuner.settings = TuneSettings {
            amount: v.get(ParamId::Correction) / 100.0,
            retune_s,
            humanize: v.get(ParamId::Humanize) / 100.0,
            vibrato: v.get(ParamId::Vibrato) / 100.0,
            mask,
            exact,
        };
    }

    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8) {
        self.notes.note_on(channel, note, velocity);
        self.update_tune();
    }

    pub fn note_off(&mut self, channel: u8, note: u8) {
        self.notes.note_off(channel, note);
        self.update_tune();
    }

    pub fn all_notes_off(&mut self) {
        self.notes.clear();
        self.update_tune();
    }

    /// Process a stereo block in place. A mono host passes the same
    /// signal on both sides and keeps the left one. Real-time safe.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        debug_assert_eq!(left.len(), right.len());
        let [dl, dr] = &mut self.dry;
        #[allow(clippy::cast_possible_truncation)]
        let sr = self.sample_rate as f32;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g_in = self.in_gain.tick();
            let g_out = self.out_gain.tick();
            let mix = self.mix.tick();
            let byp = self.bypass.tick();
            self.retuner.transpose = f64::from(self.pitch.tick());
            let formant = semitones_to_ratio(self.formant.tick());
            let (in_l, in_r) = (*l * g_in, *r * g_in);
            self.detector.push_sample(0.5 * (in_l + in_r));
            let period = self.detector.estimate().map(|e| sr / e.hz);
            let (dry_l, dry_r) = (dl.tick(*l), dr.tick(*r));
            let (wet_l, wet_r) =
                self.shifter
                    .tick_with(in_l, in_r, period, &mut self.retuner, formant);
            let out_l = (dry_l + (wet_l - dry_l) * mix) * g_out;
            let out_r = (dry_r + (wet_r - dry_r) * mix) * g_out;
            *l = out_l + (dry_l - out_l) * byp;
            *r = out_r + (dry_r - out_r) * byp;
        }
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
        v.set(ParamId::Mode, 2.0);
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
        assert_eq!(e.mode(), VoiceMode::Creative);
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
}
