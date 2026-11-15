//! Editor glue: `quack_ui::link` drawn in a truce-egui window.

use std::sync::Arc;

use quack_ui::link::{LINK_H, LINK_W, LinkAccess, LinkUi};
use truce::prelude::*;
use truce_core::editor::{PluginContext, PluginContextReadF64};
use truce_egui::EguiEditor;

use crate::LinkParams;

const ROLE: u32 = 0;
const LEVEL: u32 = 1;
const LINK: u32 = 3;
const LEVEL_MIN: f64 = -24.0;
const LEVEL_MAX: f64 = 12.0;

struct Host<'a>(&'a PluginContext<LinkParams>);

impl LinkAccess for Host<'_> {
    fn role(&self) -> usize {
        usize::from(PluginContextReadF64::get_param_plain(self.0, ROLE) >= 0.5)
    }

    fn set_role(&self, role: usize) {
        self.0.begin_edit(ROLE);
        self.0.set_param(ROLE, if role == 0 { 0.0 } else { 1.0 });
        self.0.end_edit(ROLE);
    }

    fn level_db(&self) -> f64 {
        PluginContextReadF64::get_param_plain(self.0, LEVEL)
    }

    fn begin_level(&self) {
        self.0.begin_edit(LEVEL);
    }

    fn set_level_db(&self, db: f64) {
        let n = (db.clamp(LEVEL_MIN, LEVEL_MAX) - LEVEL_MIN) / (LEVEL_MAX - LEVEL_MIN);
        self.0.set_param(LEVEL, n);
    }

    fn end_level(&self) {
        self.0.end_edit(LEVEL);
    }

    fn link(&self) -> usize {
        usize::from(PluginContextReadF64::get_param_plain(self.0, LINK) >= 0.5)
    }

    fn set_link(&self, link: usize) {
        self.0.begin_edit(LINK);
        self.0.set_param(LINK, if link == 0 { 0.0 } else { 1.0 });
        self.0.end_edit(LINK);
    }
}

/// Editor state and the last host context, so a drag left open when
/// the plugin goes away is closed.
struct Session {
    st: LinkUi,
    ctx: Option<PluginContext<LinkParams>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let (Some(ctx), true) = (&self.ctx, self.st.dragging()) {
            ctx.end_edit(LEVEL);
        }
    }
}

pub fn build(params: Arc<LinkParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    let mut session = Session {
        st: LinkUi::default(),
        ctx: None,
    };
    EguiEditor::new(params, (LINK_W, LINK_H), move |ui, ctx| {
        // A new context (first frame, window reopened): theme it, and
        // draw from the next frame, once the fonts are bound.
        if quack_ui::theme::ensure(ui.ctx(), truce_font::JETBRAINS_MONO) {
            ui.ctx().request_repaint();
            return;
        }
        quack_ui::link::draw(ui, &Host(ctx), &shared.view(), &mut session.st);
        // The meter moves without host events: keep drawing.
        ui.ctx().request_repaint();
        if session.ctx.is_none() {
            session.ctx = Some(ctx.clone());
        }
    })
    .with_visuals(truce_egui::theme::dark())
    .with_font(truce_font::JETBRAINS_MONO)
    .into_editor()
}
