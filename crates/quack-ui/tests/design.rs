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
    for (mode, name) in [(0.0, "natural"), (2.0, "creative")] {
        let mut v = ParamValues::default();
        v.set(ParamId::Mode, mode);
        v.set(ParamId::InputGain, 3.0);
        v.set(ParamId::Mix, 80.0);
        let params = Values(RefCell::new(v));
        let view = View {
            held_notes: (1 << 57) | (1 << 64),
            latency_samples: 480,
            sample_rate: 48_000.0,
        };
        let ctx = egui::Context::default();
        theme::apply(&ctx, truce_font::JETBRAINS_MONO);
        let mut st = UiState::default();
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
