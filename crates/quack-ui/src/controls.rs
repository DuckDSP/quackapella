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
#[allow(dead_code)]
pub(crate) fn knob(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    id: ParamId,
    centre: egui::Pos2,
    accent: Color32,
    fmt: &dyn Fn(f64) -> String,
) {
    knob_sized(ui, params, st, id, centre, 84.0, accent, fmt);
}

/// [`knob`] at another size (points across).
#[allow(clippy::too_many_arguments)]
pub(crate) fn knob_sized(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    id: ParamId,
    centre: egui::Pos2,
    size_pt: f32,
    accent: Color32,
    fmt: &dyn Fn(f64) -> String,
) {
    let spec = id.spec();
    let kind = spec.kind;
    let plain = params.get_plain(id);
    let area = Rect::from_center_size(centre, vec2(size_pt, size_pt));
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
    paint_knob(p, centre, size_pt, t, accent);
    knob_value(p, area, &fmt(params.get_plain(id)), spec.unit, size::VALUE);
    text(
        p,
        centre + vec2(0.0, size_pt * 0.5 + 12.0),
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

/// Previous / next choice of an enum parameter, its name in between.
/// A click on either arrow (or the wheel) is one edit; it wraps.
pub(crate) fn stepper(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    id: ParamId,
    r: Rect,
    name: &str,
    accent: Color32,
) {
    let kind = id.spec().kind;
    let count = kind.max() + 1.0;
    let at = params.get_plain(id);
    let p = ui.painter();
    rounded(p, r, radius::CONTROL, color::WELL, color::BORDER);
    let arrow_w = r.height();
    let step = |dir: f64| set_once(params, id, (at + dir).rem_euclid(count));
    for (i, (label, dir)) in [("‹", -1.0), ("›", 1.0)].into_iter().enumerate() {
        let a = if i == 0 {
            Rect::from_min_size(r.min, vec2(arrow_w, r.height()))
        } else {
            Rect::from_min_size(
                pos2(r.right() - arrow_w, r.top()),
                vec2(arrow_w, r.height()),
            )
        };
        let resp = ui.interact(
            a,
            ui.id().with(("quack-step", id.index(), i)),
            Sense::click(),
        );
        if resp.clicked() {
            step(dir);
        }
        if resp.hovered() {
            rounded(
                p,
                a.shrink(2.0),
                radius::CONTROL - 1,
                color::RAISED,
                color::RAISED,
            );
        }
        text(
            p,
            a.center(),
            Align2::CENTER_CENTER,
            label,
            size::TITLE,
            color::TEXT_DIM,
        );
    }
    let mid = Rect::from_min_max(
        pos2(r.left() + arrow_w, r.top()),
        pos2(r.right() - arrow_w, r.bottom()),
    );
    let resp = ui.interact(
        mid,
        ui.id().with(("quack-step", id.index(), 2)),
        Sense::click(),
    );
    if resp.clicked() {
        step(1.0);
    } else if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            step(-f64::from(scroll.signum()));
        }
    }
    text(
        p,
        mid.center(),
        Align2::CENTER_CENTER,
        name,
        size::BODY,
        accent,
    );
}

/// One octave of note switches, drawn as a keyboard. `scale` is the
/// key's scale (bit 0 = C): notes outside it are dark and do nothing;
/// inside it a click switches the note on or off. `active` (the notes
/// the autotune uses right now, MIDI included) get a dot.
pub(crate) fn note_switches(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    ids: &[ParamId; 12],
    r: Rect,
    scale: u16,
    active: u16,
    accent: Color32,
) {
    const WHITES: [usize; 7] = [0, 2, 4, 5, 7, 9, 11];
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let p = ui.painter();
    let ww = r.width() / 7.0;
    let mut rects = [Rect::NOTHING; 12];
    for (i, &pc) in WHITES.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let x = r.left() + ww * i as f32;
        rects[pc] = Rect::from_min_size(pos2(x, r.top()), vec2(ww - 2.0, r.height()));
    }
    for pc in [1usize, 3, 6, 8, 10] {
        let left = rects[pc - 1];
        rects[pc] = Rect::from_min_size(
            pos2(left.right() + 1.0 - ww * 0.32, r.top()),
            vec2(ww * 0.64, r.height() * 0.58),
        );
    }
    // Whites first, blacks on top; blacks take the clicks they cover.
    let order: Vec<usize> = WHITES.iter().copied().chain([1, 3, 6, 8, 10]).collect();
    let mut hit = [false; 12];
    let pointer = ui.input(|i| i.pointer.interact_pos());
    for pc in [1usize, 3, 6, 8, 10] {
        let resp = ui.interact(rects[pc], ui.id().with(("quack-note", pc)), Sense::click());
        hit[pc] = resp.clicked();
    }
    for &pc in &WHITES {
        let resp = ui.interact(rects[pc], ui.id().with(("quack-note", pc)), Sense::click());
        let on_black =
            pointer.is_some_and(|q| [1usize, 3, 6, 8, 10].iter().any(|&b| rects[b].contains(q)));
        hit[pc] = resp.clicked() && !on_black;
    }
    for pc in order {
        let in_scale = scale & (1 << pc) != 0;
        let on = params.get_plain(ids[pc]) >= 0.5;
        if hit[pc] && in_scale {
            set_once(params, ids[pc], if on { 0.0 } else { 1.0 });
        }
        let black = matches!(pc, 1 | 3 | 6 | 8 | 10);
        let k = rects[pc];
        let (fill, border, label) = match (in_scale, on) {
            (true, true) => (
                accent.gamma_multiply(0.30),
                accent.gamma_multiply(0.9),
                accent,
            ),
            (true, false) => (color::RAISED, color::BORDER, color::TEXT_DIM),
            (false, _) => (
                color::WELL,
                color::GRID_STRONG,
                color::TEXT_FAINT.gamma_multiply(0.5),
            ),
        };
        let fill = if black && !(in_scale && on) {
            color::BG
        } else {
            fill
        };
        if black {
            // Opaque under the tint: the white keys must not show through.
            rounded(p, k, radius::CONTROL - 1, color::BG, color::BG);
        }
        rounded(p, k, radius::CONTROL - 1, fill, border);
        let ty = if black {
            k.bottom() - 11.0
        } else {
            k.bottom() - 12.0
        };
        text(
            p,
            pos2(k.center().x, ty),
            Align2::CENTER_CENTER,
            NAMES[pc],
            size::CAPTION - 2.0,
            label,
        );
        if active & (1 << pc) != 0 {
            p.circle_filled(pos2(k.center().x, k.top() + 9.0), 3.0, accent);
        }
    }
}
