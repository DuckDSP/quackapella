//! Design capture of the editor, through the software rasteriser shared
//! with Sonder (`support/raster.rs`). Always renders (catches panics);
//! `QUACK_WRITE_CAPTURES=1 cargo test -p quack-ui --test design`
//! rewrites `docs/design/editor-*.png`.

mod support;

use std::cell::RefCell;

use quack_dsp::ParamId;
use quack_dsp::params::ParamValues;
use quack_ui::theme::{self, color};
use quack_ui::{BASE_H, BASE_W, ParamAccess, UiState, View, draw};

struct Values(RefCell<ParamValues>);

impl ParamAccess for Values {
    fn get_plain(&self, id: ParamId) -> f64 {
        self.0.borrow().get(id)
    }
    fn begin_edit(&self, _: ParamId) {}
    fn set_plain(&self, id: ParamId, plain: f64) {
        self.0.borrow_mut().set(id, plain);
    }
    fn end_edit(&self, _: ParamId) {}
}

#[test]
fn editor_captures() {
    let write = std::env::var_os("QUACK_WRITE_CAPTURES").is_some();
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design");
    for (mode, name) in [
        (0.0, "natural"),
        (1.0, "tune"),
        (2.0, "creative"),
        (3.0, "synth"),
    ] {
        let mut v = ParamValues::default();
        v.set(ParamId::Mode, mode);
        v.set(ParamId::InputGain, 3.0);
        v.set(ParamId::Mix, 80.0);
        v.set(ParamId::Pitch, if mode == 0.0 { -5.0 } else { 12.0 });
        v.set(ParamId::Fine, if mode == 0.0 { 12.0 } else { 0.0 });
        v.set(ParamId::Formant, if mode == 2.0 { 4.5 } else { 0.0 });
        if mode == 2.0 {
            v.set(ParamId::Pitch, -7.0);
            v.set(ParamId::FormantFollow, 80.0);
            v.set(ParamId::Sub, 45.0);
            v.set(ParamId::OctaveDown, 70.0);
            v.set(ParamId::Robot, 0.0);
            v.set(ParamId::Glitch, 25.0);
            v.set(ParamId::GlitchMode, 1.0);
            v.set(ParamId::Freeze, 1.0);
        }
        if mode == 3.0 {
            v.set(ParamId::Pitch, 0.0);
            v.set(ParamId::Fine, 0.0);
            v.set(ParamId::Harmony, 3.0);
            v.set(ParamId::Harmony3, 16.0);
            v.set(ParamId::Lead, 90.0);
        }
        if mode == 1.0 {
            v.set(ParamId::Pitch, 0.0);
            v.set(ParamId::Fine, 0.0);
            v.set(ParamId::Retune, 0.0);
            v.set(ParamId::Vibrato, 0.0);
            v.set(ParamId::Key, 9.0);
            v.set(ParamId::Scale, 2.0);
            v.set(ParamId::NoteF, 0.0);
            v.set(ParamId::Source, 1.0);
        }
        let params = Values(RefCell::new(v));
        let view = View {
            held_notes: if mode == 3.0 {
                (1 << 57) | (1 << 60) | (1 << 64)
            } else {
                (1 << 57) | (1 << 64)
            },
            latency_samples: 591,
            sample_rate: 48_000.0,
            pitch_hz: if mode == 2.0 { 0.0 } else { 223.1 },
            pitch_confidence: 0.92,
            tune_sung: 57.24,
            tune_target: 57.0,
            tune_out: if mode == 1.0 { 57.0 } else { 57.1 },
            tune_mask: if mode == 1.0 { 0b1010_1001_1101 } else { 0xFFF },
            frozen: mode == 2.0,
            harmony_notes: if mode == 3.0 {
                (1 << 52) | (1 << 60) | (1 << 64)
            } else {
                0
            },
        };
        let ctx = egui::Context::default();
        theme::apply(&ctx, truce_font::JETBRAINS_MONO);
        let mut st = UiState::default();
        // Four seconds of a phrase in the graph.
        for i in 0..240 {
            let t = f64::from(i) / 60.0;
            let mut v = view;
            let (note, len) = [
                (57.0, 0.9),
                (60.0, 0.6),
                (64.0, 1.2),
                (62.0, 0.6),
                (60.0, 0.7),
            ]
            .iter()
            .scan(0.0, |acc, &(n, l)| {
                *acc += l;
                Some((n, *acc))
            })
            .find(|&(_, end)| t < end)
            .unwrap_or((57.0, 9.0));
            let gap = (len - t) < 0.08;
            #[allow(clippy::cast_possible_truncation)]
            let wob = (0.25 * (t * 33.0).sin() + 0.2 * ((t * 7.0).sin())) as f32;
            #[allow(clippy::cast_possible_truncation)]
            let note = note as f32;
            if mode != 2.0 && !gap {
                v.tune_sung = note + 0.3 + wob;
                v.tune_target = note;
                v.tune_out = if mode == 1.0 {
                    note
                } else {
                    note + 0.6 * wob * 0.5
                };
                v.pitch_hz = quack_dsp::pitch::midi_to_hz(v.tune_sung);
            } else {
                v.tune_sung = 0.0;
                v.tune_target = 0.0;
                v.tune_out = 0.0;
                v.pitch_hz = 0.0;
            }
            if i == 239 {
                v = view;
            }
            st.record(&v, t + 1.0);
        }
        #[allow(clippy::cast_precision_loss)]
        let canvas = support::raster::render(
            &ctx,
            BASE_W as f32,
            BASE_H as f32,
            2.0,
            3,
            color::BG,
            |ui| draw(ui, &params, &view, &mut st),
        );
        if write {
            support::raster::write_png(&canvas, &out.join(format!("editor-{name}.png")))
                .expect("write capture");
        }
    }
}
