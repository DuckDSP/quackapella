//! Plugin adapter. Responsibilities, and nothing else:
//! - declare the host parameters mirroring `quack_dsp::params`;
//! - push changed host values into the engine;
//! - hand MIDI notes to the engine at their sample offsets;
//! - copy host buffers into preallocated stereo scratch and back;
//! - forward lifecycle (sample rate, latency) and publish what the
//!   editor shows;
//! - drive the snapshot morph (it writes parameters) and send the
//!   MIDI output (host port, and a virtual source on macOS).
//!
//! No DSP and no smoothing here: both belong to `quack-dsp`.

use std::sync::Arc;

use quack_dsp::Engine;
use quack_dsp::ParamId;
use quack_dsp::midiout::{MidiBuf, OutSource, Reading, Tracker};
use quack_dsp::params::ParamValues;
use quack_dsp::snapshot::{Glide, SnapshotKeys, in_snapshot, morph_value};
use truce::prelude::*;
use truce_core::events::EventBody;

mod editor;
pub mod params;
pub mod shared;
pub mod vport;

pub use params::{ModeParam, QuackParams};

/// Fixed scratch size. Larger host blocks are processed in several
/// passes, so `process` never allocates.
pub const SCRATCH_FRAMES: usize = 1024;

/// MIDI CC numbers handled here.
/// Sustain pedal.
const CC_SUSTAIN: u8 = 64;
const CC_ALL_SOUND_OFF: u8 = 120;
const CC_ALL_NOTES_OFF: u8 = 123;

/// Audio-thread state. No locks, no allocation after `reset`.
pub struct QuackDsp {
    engine: Engine,
    pushed: ParamValues,
    /// Set until the first `process` after a `reset`: the first block
    /// adopts the host values without a ramp.
    needs_snap: bool,
    left: Box<[f32; SCRATCH_FRAMES]>,
    right: Box<[f32; SCRATCH_FRAMES]>,
    /// Side-chain input (the vocoder's external carrier).
    sc_left: Box<[f32; SCRATCH_FRAMES]>,
    sc_right: Box<[f32; SCRATCH_FRAMES]>,
    sample_rate: f64,
    morph: Morph,
    tracker: Tracker,
    midi: MidiBuf,
    midi_on: bool,
    vport: vport::VirtualPort,
}

/// Where the morph stands (see `quack_dsp::snapshot`).
#[derive(Debug, Default)]
struct Morph {
    /// Morph value last written to the parameters (`None`: write on the
    /// next block).
    last: Option<f64>,
    /// Snapshot generation that was written.
    generation: u32,
    glide: Option<Glide>,
}

impl Default for QuackDsp {
    fn default() -> Self {
        Self {
            engine: Engine::new(),
            pushed: ParamValues::default(),
            needs_snap: true,
            left: Box::new([0.0; SCRATCH_FRAMES]),
            right: Box::new([0.0; SCRATCH_FRAMES]),
            sc_left: Box::new([0.0; SCRATCH_FRAMES]),
            sc_right: Box::new([0.0; SCRATCH_FRAMES]),
            sample_rate: 48_000.0,
            morph: Morph::default(),
            tracker: Tracker::default(),
            midi: MidiBuf::default(),
            midi_on: false,
            vport: vport::VirtualPort::default(),
        }
    }
}

impl QuackDsp {
    fn push_changes(&mut self, target: &ParamValues) {
        for id in quack_dsp::ParamId::ALL {
            let v = target.get(id);
            if v != self.pushed.get(id) {
                self.engine.set_param(id, v);
                self.pushed.set(id, v);
            }
        }
    }

    /// Go to snapshot `slot`: glide the morph there with Morph On,
    /// recall it at once otherwise.
    fn go_to(&mut self, slot: usize, params: &QuackParams) {
        let v = params.to_values();
        if v.get(ParamId::MorphOn) >= 0.5 {
            let from = v.get(ParamId::Morph);
            let to = if slot == 0 { 0.0 } else { 100.0 };
            let samples = v.get(ParamId::MorphTime) * 1e-3 * self.sample_rate;
            self.morph.glide = Some(Glide::new(from, to, samples));
        } else if let Some(snap) = params.control.snapshot(slot) {
            for id in ParamId::ALL {
                if in_snapshot(id) {
                    set_plain(params, id, snap.get(id));
                }
            }
        }
    }

    /// Once per block, before the engine reads the values: advance a
    /// glide, and write the morphed parameters when the morph moved.
    fn update_morph(&mut self, params: &QuackParams, frames: usize) {
        if let Some(slot) = params.control.take_snapshot_request() {
            self.go_to(slot, params);
        }
        if params.to_values().get(ParamId::MorphOn) < 0.5 {
            self.morph = Morph::default();
            return;
        }
        if let Some(g) = &mut self.morph.glide {
            set_plain(params, ParamId::Morph, g.advance(frames));
            if g.done() {
                self.morph.glide = None;
            }
        }
        // The macros may move the morph too.
        let m = params.engine_values().get(ParamId::Morph);
        let generation = params.control.snapshot_gen();
        if self.morph.last == Some(m) && self.morph.generation == generation {
            return;
        }
        self.morph.last = Some(m);
        self.morph.generation = generation;
        let (Some(a), Some(b)) = (params.control.snapshot(0), params.control.snapshot(1)) else {
            return;
        };
        for id in ParamId::ALL {
            if in_snapshot(id) && a.get(id) != b.get(id) {
                set_plain(params, id, morph_value(id, &a, &b, m / 100.0));
            }
        }
    }

    /// After the audio: the MIDI output of this block.
    fn update_midi_out(&mut self, v: &ParamValues, peak: f32, frames: usize) {
        self.midi.clear();
        let on = v.get(ParamId::MidiOut) >= 0.5;
        if on {
            let pitch = self.engine.pitch();
            let tune = self.engine.tune();
            #[allow(clippy::cast_possible_truncation)]
            let reading = Reading {
                sung: pitch.map(|p| p.note()),
                target: tune.map(|t| t.target),
                transpose: (v.get(ParamId::Pitch) + v.get(ParamId::Fine) / 100.0) as f32,
                harmony: self.engine.harmony_notes(),
                peak,
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let channel = v.get(ParamId::MidiOutChannel) as u8;
            self.tracker.update(
                &reading,
                OutSource::from_index(v.get(ParamId::MidiOutSource)),
                channel,
                frames,
                &mut self.midi,
            );
        } else if self.midi_on {
            self.tracker.all_off(&mut self.midi);
        }
        for m in self.midi.as_slice() {
            self.vport.send(*m);
        }
        // Switch the virtual source after the last note-offs went out.
        self.vport.set_enabled(on);
        self.midi_on = on;
    }

    /// Apply one MIDI event. Returns whether it moved parameters (a
    /// learned CC or a program change): the caller re-reads them.
    fn handle(&mut self, body: &EventBody, params: &QuackParams) -> bool {
        let keys = SnapshotKeys::from_index(params.to_values().get(ParamId::SnapshotKeys));
        match *body {
            // The snapshot notes go no further.
            EventBody::NoteOn { note, velocity, .. } if keys.slot(note).is_some() => {
                if velocity > 0
                    && let Some(slot) = keys.slot(note)
                {
                    self.go_to(slot, params);
                    return true;
                }
            }
            EventBody::NoteOff { note, .. } if keys.slot(note).is_some() => {}
            EventBody::NoteOn {
                channel,
                note,
                velocity,
                ..
            } => self.engine.note_on(channel, note, velocity),
            EventBody::NoteOff { channel, note, .. } => self.engine.note_off(channel, note),
            EventBody::ControlChange { cc, .. }
                if cc == CC_ALL_NOTES_OFF || cc == CC_ALL_SOUND_OFF =>
            {
                self.engine.all_notes_off();
            }
            EventBody::PitchBend { value, .. } => {
                self.engine.pitch_bend((f64::from(value) - 8192.0) / 8192.0);
            }
            EventBody::ControlChange { cc, value, .. } if cc == CC_SUSTAIN => {
                self.engine.set_sustain(value >= 64);
            }
            // MIDI learn: the CC moves the parameter itself, so the
            // editor and the saved state follow.
            EventBody::ControlChange { cc, value, .. } => {
                if let Some((id, norm)) = params.control.on_cc(cc, value) {
                    params.set_normalized(id.spec().host_id, norm);
                    return true;
                }
            }
            EventBody::ProgramChange { program, .. } if params.control.program_change() => {
                return params.recall_factory(usize::from(program));
            }
            _ => {}
        }
        false
    }

    /// Run the engine on `frames` of the buffer from `offset`, through
    /// the scratch (mono in feeds both sides). The main input has as
    /// many channels as the output; input channels past them are the
    /// side-chain.
    fn run(&mut self, buffer: &mut AudioBuffer, offset: usize, frames: usize) {
        let (ins, outs) = (buffer.num_input_channels(), buffer.num_output_channels());
        let main = outs.min(ins);
        let sc = ins - main;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(SCRATCH_FRAMES);
            let at = offset + done;
            let (l, r) = (&mut self.left[..n], &mut self.right[..n]);
            l.copy_from_slice(&buffer.input(0)[at..at + n]);
            if main > 1 {
                r.copy_from_slice(&buffer.input(1)[at..at + n]);
            } else {
                r.copy_from_slice(l);
            }
            let side = if sc > 0 {
                let (a, b) = (&mut self.sc_left[..n], &mut self.sc_right[..n]);
                a.copy_from_slice(&buffer.input(main)[at..at + n]);
                if sc > 1 {
                    b.copy_from_slice(&buffer.input(main + 1)[at..at + n]);
                } else {
                    b.copy_from_slice(a);
                }
                Some((&a[..], &b[..]))
            } else {
                None
            };
            self.engine.process_with_sidechain(l, r, side);
            buffer.output(0)[at..at + n].copy_from_slice(l);
            if outs > 1 {
                buffer.output(1)[at..at + n].copy_from_slice(r);
            }
            done += n;
        }
    }
}

/// Write a plain value to a host parameter (from the audio thread, as
/// MIDI learn does).
fn set_plain(params: &QuackParams, id: ParamId, plain: f64) {
    let spec = id.spec();
    params.set_normalized(spec.host_id, spec.kind.normalize(plain));
}

/// A MIDI-out message as a host event.
fn to_event(m: quack_dsp::midiout::Msg) -> Option<EventBody> {
    let channel = m.status & 15;
    Some(match m.status & 0xF0 {
        0x90 => EventBody::NoteOn {
            group: 0,
            channel,
            note: m.data1,
            velocity: m.data2,
        },
        0x80 => EventBody::NoteOff {
            group: 0,
            channel,
            note: m.data1,
            velocity: m.data2,
        },
        0xE0 => EventBody::PitchBend {
            group: 0,
            channel,
            value: u16::from(m.data1) | u16::from(m.data2) << 7,
        },
        _ => return None,
    })
}

pub struct Quackapella;

impl PluginLogic for Quackapella {
    type Params = QuackParams;
    type DspState = QuackDsp;

    /// Stereo or mono, each with a stereo side-chain input: the
    /// vocoder's external carrier (AU wires a single side-chain width).
    fn bus_layouts() -> Vec<BusLayout> {
        vec![
            BusLayout::stereo().with_sidechain_input("Sidechain", ChannelConfig::Stereo),
            BusLayout::mono().with_sidechain_input("Sidechain", ChannelConfig::Stereo),
        ]
    }

    fn reset(state: &mut QuackDsp, params: &QuackParams, config: &AudioConfig) {
        state
            .engine
            .prepare(config.sample_rate, config.max_block_size.max(1));
        state.engine.set_taps(params.shared.taps.clone());
        state.needs_snap = true;
        state.sample_rate = config.sample_rate;
        state.tracker = Tracker::default();
        state.tracker.set_sample_rate(config.sample_rate);
        state.midi_on = false;
        state.morph = Morph::default();
        state.vport.start();
        #[allow(clippy::cast_possible_truncation)]
        params
            .shared
            .publish_config(state.engine.latency_samples() as u32, config.sample_rate);
        params.shared.publish_notes(0);
        params.shared.publish_pitch(None);
        params.shared.publish_tune(None, state.engine.tune_mask());
    }

    fn process(
        state: &mut QuackDsp,
        params: &QuackParams,
        buffer: &mut AudioBuffer,
        events: &EventList,
        context: &mut ProcessContext,
    ) -> ProcessStatus {
        let t = context.transport;
        let bar = if t.time_sig_num > 0 && t.time_sig_den > 0 {
            f64::from(t.time_sig_num) * 4.0 / f64::from(t.time_sig_den)
        } else {
            4.0
        };
        state
            .engine
            .set_transport(t.tempo, bar, t.playing.then_some(t.position_beats));
        let total = buffer.num_samples();
        state.update_morph(params, total);
        // Host values once per (sub-)block: truce splits blocks at
        // sample-accurate parameter events, the engine ramps from here.
        let target = params.engine_values();
        if state.needs_snap {
            state.engine.set_all(&target, true);
            state.pushed = target;
            state.needs_snap = false;
        } else {
            state.push_changes(&target);
        }

        let (ins, outs) = (buffer.num_input_channels(), buffer.num_output_channels());
        if ins == 0 || outs == 0 {
            for e in events.iter() {
                if state.handle(&e.body, params) {
                    state.push_changes(&params.engine_values());
                }
            }
            return ProcessStatus::Normal;
        }
        let peak = buffer.input(0)[..total]
            .iter()
            .fold(0.0_f32, |m, x| m.max(x.abs()));
        // MIDI at its sample offset: run up to each event, then apply it.
        let mut pos = 0;
        for e in events.iter() {
            let at = (e.sample_offset as usize).min(total);
            if at > pos {
                state.run(buffer, pos, at - pos);
                pos = at;
            }
            if state.handle(&e.body, params) {
                state.push_changes(&params.engine_values());
            }
        }
        if pos < total {
            state.run(buffer, pos, total - pos);
        }
        state.update_midi_out(&params.engine_values(), peak, total);
        #[allow(clippy::cast_possible_truncation)]
        let at = total.saturating_sub(1) as u32;
        for m in state.midi.as_slice() {
            if let Some(body) = to_event(*m) {
                context.output_events.push(Event {
                    sample_offset: at,
                    port: 0,
                    body,
                });
            }
        }
        params.shared.publish_midi_out(state.tracker.sounding());
        params.shared.publish_notes(state.engine.notes().mask());
        #[allow(clippy::cast_possible_truncation)]
        params
            .shared
            .publish_latency(state.engine.latency_samples() as u32);
        params.shared.publish_pitch(state.engine.pitch());
        params
            .shared
            .publish_tune(state.engine.tune(), state.engine.tune_mask());
        params.shared.publish_frozen(state.engine.frozen());
        params.shared.publish_harmony(state.engine.harmony_notes());
        params.shared.publish_vocoder(state.engine.vocoder_notes());
        params
            .shared
            .publish_reduction(state.engine.fx_reduction_db());
        params
            .shared
            .publish_output_reduction(state.engine.output_reduction_db());
        ProcessStatus::Normal
    }

    fn latency(state: &QuackDsp) -> u32 {
        #[allow(clippy::cast_possible_truncation)]
        let n = state.engine.latency_samples() as u32;
        n
    }

    fn editor(params: Arc<QuackParams>) -> Box<dyn Editor> {
        editor::build(params)
    }
}

truce::plugin! {
    logic: Quackapella,
    params: QuackParams,
}

truce::enable_rt_paranoid!();
