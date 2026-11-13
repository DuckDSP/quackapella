//! Drawing helpers in the DuckDSP style (after Sonder's `paint.rs`):
//! flat surfaces, thin outlines, one accent at a time.

use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

use crate::theme::{color, label_font, radius, size, value_font};

pub(crate) fn text(p: &Painter, pos: Pos2, align: Align2, s: &str, size: f32, c: Color32) -> Rect {
    p.text(pos, align, s, label_font(size), c)
}

/// Text held within `max_w`: shrunk a little first, then cut short
/// with an ellipsis. Nothing may run into its neighbour.
pub(crate) fn text_fit(
    p: &Painter,
    pos: Pos2,
    align: Align2,
    s: &str,
    size: f32,
    c: Color32,
    max_w: f32,
) -> Rect {
    let width = |t: &str, sz: f32| p.layout_no_wrap(t.to_owned(), label_font(sz), c).size().x;
    let mut sz = size;
    while width(s, sz) > max_w && sz > (size - 2.0).max(9.0) {
        sz -= 0.5;
    }
    if width(s, sz) <= max_w {
        return p.text(pos, align, s, label_font(sz), c);
    }
    let mut cut: String = s.to_owned();
    loop {
        cut.pop();
        let t = format!("{}…", cut.trim_end());
        if cut.is_empty() || width(&t, sz) <= max_w {
            return p.text(pos, align, t, label_font(sz), c);
        }
    }
}

pub(crate) fn value(p: &Painter, pos: Pos2, align: Align2, s: &str, size: f32, c: Color32) -> Rect {
    p.text(pos, align, s, value_font(size), c)
}

/// Text with extra space between letters (the wordmark).
pub(crate) fn spaced(p: &Painter, pos: Pos2, s: &str, size: f32, track: f32, c: Color32) -> Rect {
    let mut x = pos.x;
    let mut r = Rect::NOTHING;
    for ch in s.chars() {
        let g = p.text(pos2(x, pos.y), Align2::LEFT_CENTER, ch, label_font(size), c);
        r = r.union(g);
        x = g.right() + track;
    }
    r
}

pub(crate) fn rounded(p: &Painter, r: Rect, rad: u8, fill: Color32, border: Color32) {
    p.rect(
        r,
        CornerRadius::same(rad),
        fill,
        Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    );
}

pub(crate) fn panel(p: &Painter, r: Rect) {
    rounded(p, r, radius::PANEL, color::PANEL, color::BORDER);
}

pub(crate) fn well(p: &Painter, r: Rect) {
    rounded(p, r, radius::CONTROL, color::WELL, color::GRID_STRONG);
}

/// Rotary control in Knock's style (`knob_face_sized` there): dark
/// face, thin neutral track over 270°, thin value arc in the accent, a
/// faint inner ring and no pointer: the value is written inside (see
/// [`knob_value`]). `size` is the outer diameter.
pub(crate) fn knob(p: &Painter, c: Pos2, size: f32, t: f32, accent: Color32) {
    let scale = size / 64.0;
    let radius = 28.0 * scale;
    p.circle_filled(c, 32.0 * scale, color::BG);
    let arc = |end: f32| -> Vec<Pos2> {
        (0..=48)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let a = std::f32::consts::PI * (0.75 + 1.5 * end * i as f32 / 48.0);
                c + vec2(a.cos(), a.sin()) * radius
            })
            .collect()
    };
    p.add(Shape::line(arc(1.0), Stroke::new(2.0_f32, color::BORDER)));
    p.add(Shape::line(
        arc(t.clamp(0.0, 1.0)),
        Stroke::new(2.0_f32, accent),
    ));
    p.circle_stroke(
        c,
        24.5 * scale,
        Stroke::new(1.0_f32, color::BORDER.gamma_multiply(0.7)),
    );
}

/// The mixing assistant's proposal on a knob: a dot on the track where
/// the value would go.
pub(crate) fn hint_dot(p: &Painter, c: Pos2, size: f32, t: f32) {
    let scale = size / 64.0;
    let ang = std::f32::consts::PI * (0.75 + 1.5 * t.clamp(0.0, 1.0));
    let at = c + vec2(ang.cos(), ang.sin()) * 28.0 * scale;
    p.circle_filled(at, 4.5 * scale.max(0.8), color::BG);
    p.circle_filled(at, 3.0 * scale.max(0.8), color::TEXT);
}

/// Macro modulation on a knob: a thin outer arc from the set value `t`
/// to the modulated one `m`, and a dot where the macros take it.
pub(crate) fn mod_arc(p: &Painter, c: Pos2, size: f32, t: f32, m: f32, accent: Color32) {
    let scale = size / 64.0;
    let radius = 31.5 * scale;
    let (a, b) = (t.clamp(0.0, 1.0), m.clamp(0.0, 1.0));
    let at = |u: f32| {
        let ang = std::f32::consts::PI * (0.75 + 1.5 * u);
        c + vec2(ang.cos(), ang.sin()) * radius
    };
    let pts: Vec<Pos2> = (0..=24)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let u = a + (b - a) * i as f32 / 24.0;
            at(u)
        })
        .collect();
    p.add(Shape::line(
        pts,
        Stroke::new(1.5_f32, color::TEXT.gamma_multiply(0.7)),
    ));
    p.circle_filled(at(b), 2.5 * scale.max(0.8), accent.gamma_multiply(1.2));
}

/// The value inside a knob, as in Knock: number in the value font, unit
/// smaller and dimmer; shrunk until it fits the face.
pub(crate) fn knob_value(p: &Painter, r: Rect, val: &str, unit: &str, size: f32) {
    let mut font_size = size;
    loop {
        let mut job = egui::text::LayoutJob::default();
        job.append(
            val,
            0.0,
            egui::TextFormat {
                font_id: value_font(font_size),
                color: color::TEXT,
                ..Default::default()
            },
        );
        if !unit.is_empty() {
            job.append(
                &format!(" {unit}"),
                0.0,
                egui::TextFormat {
                    font_id: value_font(font_size - 3.0),
                    color: color::TEXT_DIM,
                    ..Default::default()
                },
            );
        }
        let galley = p.layout_job(job);
        if galley.size().x <= r.width() - 18.0 || font_size <= 9.0 {
            p.galley(r.center() - galley.size() * 0.5, galley, color::TEXT);
            break;
        }
        font_size -= 0.5;
    }
}

/// Button-like box with centred text. `on` lights it in `accent`.
pub(crate) fn pill(p: &Painter, r: Rect, label: &str, on: bool, hover: bool, accent: Color32) {
    if on {
        rounded(
            p,
            r,
            radius::CONTROL,
            accent.gamma_multiply(0.20),
            accent.gamma_multiply(0.9),
        );
    } else {
        rounded(
            p,
            r,
            radius::CONTROL,
            if hover { color::BORDER } else { color::RAISED },
            color::BORDER,
        );
    }
    let c = if on { accent } else { color::TEXT_DIM };
    text(p, r.center(), Align2::CENTER_CENTER, label, size::BODY, c);
}
