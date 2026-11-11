//! Controls bound to parameters. Every gesture is one host edit
//! (`begin_edit` at press, `end_edit` at release); a click or a
//! double-click is one edit on its own.

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use quack_dsp::ParamId;

use crate::paint::{knob as paint_knob, knob_value, pill, rounded, text};
use crate::theme::{color, radius, size};
use crate::{ParamAccess, UiState, set_once};

/// Knob travel for the whole range, in points of vertical drag.
pub const KNOB_TRAVEL: f32 = 200.0;
const WHEEL_STEP: f64 = 0.01;

/// Knob in Knock's style, value and unit written inside, label under
/// it. Drag up or right for more (Shift = fine), wheel for small steps,
/// double-click for the default.
pub(crate) fn knob(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    id: ParamId,
    centre: egui::Pos2,
    accent: Color32,
    fmt: &dyn Fn(f64) -> String,
) {
    const SIZE: f32 = 84.0;
    let spec = id.spec();
    let kind = spec.kind;
    let plain = params.get_plain(id);
    let area = Rect::from_center_size(centre, vec2(SIZE, SIZE));
    let resp = ui.interact(
        area,
        ui.id().with(("quack-knob", id.index())),
        Sense::click_and_drag(),
    );

    if resp.drag_started() {
        params.begin_edit(id);
        st.drag = Some((id, kind.normalize(plain)));
    }
    if resp.dragged()
        && let Some((d, n0)) = st.drag
        && d == id
        && let (Some(now), Some(from)) = (
            resp.interact_pointer_pos(),
            ui.input(|i| i.pointer.press_origin()),
        )
    {
        let fine = if ui.input(|i| i.modifiers.shift) {
            0.1
        } else {
            1.0
        };
        let moved = (now.x - from.x) - (now.y - from.y);
        let n = (n0 + f64::from(moved / KNOB_TRAVEL) * fine).clamp(0.0, 1.0);
        let target = kind.denormalize(n);
        if target != plain {
            params.set_plain(id, target);
        }
    }
    if resp.drag_stopped() && st.drag.is_some_and(|(d, _)| d == id) {
        st.drag = None;
        params.end_edit(id);
    }
    if resp.double_clicked() {
        set_once(params, id, kind.default_plain());
    } else if resp.hovered() && st.drag.is_none() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let n = kind.normalize(plain) + f64::from(scroll.signum()) * WHEEL_STEP;
            set_once(params, id, kind.denormalize(n));
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }

    let p = ui.painter();
    #[allow(clippy::cast_possible_truncation)]
    let t = kind.normalize(params.get_plain(id)) as f32;
    paint_knob(p, centre, SIZE, t, accent);
    knob_value(p, area, &fmt(params.get_plain(id)), spec.unit, size::VALUE);
    text(
        p,
        centre + vec2(0.0, SIZE * 0.5 + 12.0),
        Align2::CENTER_CENTER,
        spec.name,
        size::CAPTION,
        color::TEXT_DIM,
    );
}

/// Segmented choice for an enum parameter: a click on a segment is one
/// edit. `accents` lights each choice in its own colour.
pub(crate) fn segmented(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    id: ParamId,
    r: Rect,
    labels: &[&str],
    accents: &[Color32],
) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lit = (params.get_plain(id).max(0.0).round() as usize).min(labels.len() - 1);
    let p = ui.painter();
    rounded(p, r, radius::CONTROL, color::WELL, color::BORDER);
    #[allow(clippy::cast_precision_loss)]
    let w = r.width() / labels.len() as f32;
    for (i, label) in labels.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let seg = Rect::from_min_size(pos2(r.left() + w * i as f32, r.top()), vec2(w, r.height()));
        let resp = ui.interact(
            seg,
            ui.id().with(("quack-seg", id.index(), i)),
            Sense::click(),
        );
        if resp.clicked() && i != lit {
            #[allow(clippy::cast_precision_loss)]
            set_once(params, id, i as f64);
        }
        let accent = accents[i % accents.len()];
        if i == lit {
            rounded(
                p,
                seg.shrink(2.0),
                radius::CONTROL - 1,
                accent.gamma_multiply(0.22),
                accent.gamma_multiply(0.8),
            );
        } else if resp.hovered() {
            rounded(
                p,
                seg.shrink(2.0),
                radius::CONTROL - 1,
                color::RAISED,
                color::RAISED,
            );
        }
        let c = if i == lit { accent } else { color::TEXT_DIM };
        text(p, seg.center(), Align2::CENTER_CENTER, label, size::BODY, c);
    }
}

/// Toggle for a bool parameter.
pub(crate) fn toggle(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    id: ParamId,
    r: Rect,
    label: &str,
    accent: Color32,
) {
    let on = params.get_plain(id) >= 0.5;
    let resp = ui.interact(
        r,
        ui.id().with(("quack-toggle", id.index())),
        Sense::click(),
    );
    if resp.clicked() {
        set_once(params, id, if on { 0.0 } else { 1.0 });
    }
    pill(ui.painter(), r, label, on, resp.hovered(), accent);
}
