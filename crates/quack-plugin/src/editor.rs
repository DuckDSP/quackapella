//! Editor glue: `quack-ui` drawn in a truce-egui window, with
//! [`ParamAccess`] implemented over the host.

use std::sync::Arc;

use quack_dsp::ParamId;
use quack_ui::{BASE_H, BASE_W, ParamAccess, UiState};
use truce::prelude::*;
use truce_core::editor::{PluginContext, PluginContextReadF64};
use truce_egui::EguiEditor;

use crate::QuackParams;

/// Host-backed parameter access: plain values across the trait, the
/// host speaks normalized, converted with the registry's mapping.
pub struct HostParams<'a>(pub &'a PluginContext<QuackParams>);

impl ParamAccess for HostParams<'_> {
    fn get_plain(&self, id: ParamId) -> f64 {
        PluginContextReadF64::get_param_plain(self.0, id.spec().host_id)
    }

    fn begin_edit(&self, id: ParamId) {
        self.0.begin_edit(id.spec().host_id);
    }

    fn set_plain(&self, id: ParamId, plain: f64) {
        let kind = id.spec().kind;
        self.0.set_param(id.spec().host_id, kind.normalize(plain));
    }

    fn end_edit(&self, id: ParamId) {
        self.0.end_edit(id.spec().host_id);
    }
}

/// Editor state and the last host context, so an edit left open when
/// the plugin is destroyed mid-drag is closed.
struct Session {
    st: UiState,
    ctx: Option<PluginContext<QuackParams>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(ctx) = &self.ctx {
            self.st.end_open_edits(&HostParams(ctx));
        }
    }
}

pub fn build(params: Arc<QuackParams>) -> Box<dyn Editor> {
    let shared = params.shared.clone();
    let mut session = Session {
        st: UiState::default(),
        ctx: None,
    };
    let mut themed = false;
    EguiEditor::new(params, (BASE_W, BASE_H), move |ui, ctx| {
        if !std::mem::replace(&mut themed, true) {
            quack_ui::theme::apply(ui.ctx(), truce_font::JETBRAINS_MONO);
            // Fonts apply on the next frame, and the titles use a named
            // family egui's defaults lack: skip drawing this one.
            ui.ctx().request_repaint();
            return;
        }
        let view = shared.view();
        quack_ui::draw(ui, &HostParams(ctx), &view, &mut session.st);
        // Notes and meters move without host events: keep drawing.
        ui.ctx().request_repaint();
        if session.ctx.is_none() {
            session.ctx = Some(ctx.clone());
        }
    })
    .with_visuals(truce_egui::theme::dark())
    .with_font(truce_font::JETBRAINS_MONO)
    .into_editor()
}
