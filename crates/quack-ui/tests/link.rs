//! Design captures of Quackapella Link's editor, one per state the
//! footer can tell. `QUACK_WRITE_CAPTURES=1 cargo test -p quack-ui
//! --test link` rewrites `docs/design/link-*.png`.

mod support;

use std::cell::Cell;

use quack_ui::link::{LINK_H, LINK_W, LinkAccess, LinkState, LinkUi, LinkView, draw};
use quack_ui::theme::{self, color};

struct Values(Cell<usize>, Cell<f64>);

impl LinkAccess for Values {
    fn role(&self) -> usize {
        self.0.get()
    }
    fn set_role(&self, role: usize) {
        self.0.set(role);
    }
    fn level_db(&self) -> f64 {
        self.1.get()
    }
    fn begin_level(&self) {}
    fn set_level_db(&self, db: f64) {
        self.1.set(db);
    }
    fn end_level(&self) {}
}

#[test]
fn link_captures() {
    let write = std::env::var_os("QUACK_WRITE_CAPTURES").is_some();
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design");
    for (name, role, level, state, peak) in [
        (
            "mix",
            0,
            0.0,
            LinkState::Sending { heard: true },
            [0.32, 0.26],
        ),
        (
            "carrier",
            1,
            -3.5,
            LinkState::Sending { heard: false },
            [0.7, 0.92],
        ),
        ("busy", 1, 12.0, LinkState::Busy, [0.0, 0.0]),
        ("off", 0, -24.0, LinkState::Off, [0.0, 0.0]),
    ] {
        let ctx = egui::Context::default();
        theme::apply(&ctx, truce_font::JETBRAINS_MONO);
        let values = Values(Cell::new(role), Cell::new(level));
        let mut st = LinkUi::default();
        let mut blocks = 0;
        #[allow(clippy::cast_precision_loss)]
        let canvas = support::raster::render(
            &ctx,
            LINK_W as f32,
            LINK_H as f32,
            2.0,
            3,
            color::BG,
            |ui| {
                blocks += 1;
                let view = LinkView {
                    peak,
                    state,
                    blocks,
                };
                draw(ui, &values, &view, &mut st);
            },
        );
        if write {
            support::raster::write_png(&canvas, &out.join(format!("link-{name}.png")))
                .expect("write capture");
        }
    }
}

/// The host hands a reopened window a new context while the editor's
/// state lives on: the theme follows the context.
#[test]
fn a_new_context_gets_the_theme_again() {
    let first = egui::Context::default();
    assert!(theme::ensure(&first, truce_font::JETBRAINS_MONO));
    assert!(!theme::ensure(&first, truce_font::JETBRAINS_MONO));
    let reopened = egui::Context::default();
    assert!(theme::ensure(&reopened, truce_font::JETBRAINS_MONO));
}
