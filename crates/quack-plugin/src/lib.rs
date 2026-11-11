//! Plugin adapter. Responsibilities, and nothing else:
//! - declare the host parameters mirroring `quack_dsp::params`;
//! - push changed host values into the engine;
//! - hand MIDI notes to the engine at their sample offsets;
//! - copy host buffers into preallocated stereo scratch and back;
//! - forward lifecycle (sample rate, latency) and publish what the
//!   editor shows.
//!
//! No DSP and no smoothing here: both belong to `quack-dsp`.

use std::sync::Arc;

use quack_dsp::Engine;
use quack_dsp::params::ParamValues;
use truce::prelude::*;
use truce_core::events::EventBody;

mod editor;
pub mod params;
pub mod shared;

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
}

impl Default for QuackDsp {
    fn default() -> Self {
        Self {
            engine: Engine::new(),
            pushed: ParamValues::default(),
            needs_snap: true,
            left: Box::new([0.0; SCRATCH_FRAMES]),
            right: Box::new([0.0; SCRATCH_FRAMES]),
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

    fn handle(&mut self, body: &EventBody) {
        match *body {
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
            EventBody::ControlChange { cc, value, .. } if cc == CC_SUSTAIN => {
                self.engine.set_sustain(value >= 64);
            }
            _ => {}
        }
    }

    /// Run the engine on `frames` of the buffer from `offset`, through
    /// the scratch (mono in feeds both sides).
    fn run(&mut self, buffer: &mut AudioBuffer, offset: usize, frames: usize) {
        let (ins, outs) = (buffer.num_input_channels(), buffer.num_output_channels());
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(SCRATCH_FRAMES);
            let at = offset + done;
            let (l, r) = (&mut self.left[..n], &mut self.right[..n]);
            l.copy_from_slice(&buffer.input(0)[at..at + n]);
            if ins > 1 {
                r.copy_from_slice(&buffer.input(1)[at..at + n]);
            } else {
                r.copy_from_slice(l);
            }
            self.engine.process(l, r);
            buffer.output(0)[at..at + n].copy_from_slice(l);
            if outs > 1 {
                buffer.output(1)[at..at + n].copy_from_slice(r);
            }
            done += n;
        }
    }
}

pub struct Quackapella;

impl PluginLogic for Quackapella {
    type Params = QuackParams;
    type DspState = QuackDsp;

    fn reset(state: &mut QuackDsp, params: &QuackParams, config: &AudioConfig) {
        state
            .engine
            .prepare(config.sample_rate, config.max_block_size.max(1));
        state.needs_snap = true;
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
        // Host values once per (sub-)block: truce splits blocks at
        // sample-accurate parameter events, the engine ramps from here.
        let target = params.to_values();
        if state.needs_snap {
            state.engine.set_all(&target, true);
            state.pushed = target;
            state.needs_snap = false;
        } else {
            state.push_changes(&target);
        }

        let (ins, outs) = (buffer.num_input_channels(), buffer.num_output_channels());
        let total = buffer.num_samples();
        if ins == 0 || outs == 0 {
            for e in events.iter() {
                state.handle(&e.body);
            }
            return ProcessStatus::Normal;
        }
        // MIDI at its sample offset: run up to each event, then apply it.
        let mut pos = 0;
        for e in events.iter() {
            let at = (e.sample_offset as usize).min(total);
            if at > pos {
                state.run(buffer, pos, at - pos);
                pos = at;
            }
            state.handle(&e.body);
        }
        if pos < total {
            state.run(buffer, pos, total - pos);
        }
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
