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

    fn control(&self) -> Option<&quack_dsp::control::ControlState> {
        Some(&self.0.params().control)
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
    session.st.set_taps(shared.taps.clone());
    let mut themed = false;
    // Zoom applied to egui and the window (percent).
    let mut zoom_applied: Option<u16> = None;
    // Time of the last resize request: until the window follows (or
    // 1.5 s pass), its size is not read back as a zoom.
    let mut resize_pending: Option<f64> = None;
    EguiEditor::new(params, (BASE_W, BASE_H), move |ui, ctx| {
        if !std::mem::replace(&mut themed, true) {
            quack_ui::theme::apply(ui.ctx(), truce_font::JETBRAINS_MONO);
            // Fonts apply on the next frame, and the titles use a named
            // family egui's defaults lack: skip drawing this one.
            ui.ctx().request_repaint();
            return;
        }
        // Zoom: the saved step, or the host's own resize (CLAP / VST3
        // window edges) adopted as the new zoom.
        let zoom = ctx.params().control.zoom();
        if zoom_applied != Some(zoom) {
            let z = f32::from(zoom) / 100.0;
            ui.ctx().set_zoom_factor(z);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (w, h) = (
                (BASE_W as f32 * z).round() as u32,
                (BASE_H as f32 * z).round() as u32,
            );
            let _ = ctx.request_resize(w, h);
            zoom_applied = Some(zoom);
            resize_pending = Some(ui.input(|i| i.time));
        } else {
            #[allow(clippy::cast_precision_loss)]
            let seen = ui.ctx().content_rect().width() / BASE_W as f32;
            let fits = (seen - 1.0).abs() <= 0.02;
            let now = ui.input(|i| i.time);
            if fits || resize_pending.is_some_and(|t| now - t > 1.5) {
                resize_pending = None;
            }
            if !fits && resize_pending.is_none() && seen.is_finite() && seen > 0.0 {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let new = (f32::from(zoom) * seen).round().clamp(50.0, 200.0) as u16;
                ctx.params().control.set_zoom(new);
                ui.ctx().set_zoom_factor(f32::from(new) / 100.0);
                zoom_applied = Some(new);
            }
        }
        let view = shared.view();
        quack_ui::draw(ui, &HostParams(ctx), &view, &mut session.st);
        // Notes and meters move without host events: keep drawing.
        ui.ctx().request_repaint();
        if session.ctx.is_none() {
            session.ctx = Some(ctx.clone());
        }
    })
    .resizable(true)
    .aspect_ratio(Some((BASE_W, BASE_H)))
    .min_size((BASE_W / 2, BASE_H / 2))
    .max_size((BASE_W * 2, BASE_H * 2))
    .with_visuals(truce_egui::theme::dark())
    .with_font(truce_font::JETBRAINS_MONO)
    .into_editor()
}
