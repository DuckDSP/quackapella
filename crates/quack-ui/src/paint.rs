//! Drawing helpers in the DuckDSP style (after Sonder's `paint.rs`):
//! flat surfaces told apart by their shade (no outlines on panels and
//! controls), one accent at a time.

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

/// A filled box without outline.
pub(crate) fn fill(p: &Painter, r: Rect, rad: u8, c: Color32) {
    p.rect_filled(r, CornerRadius::same(rad), c);
}

pub(crate) fn panel(p: &Painter, r: Rect) {
    fill(p, r, radius::PANEL, color::PANEL);
}

/// A section's title: small capitals, spaced out, in `c` (faint text
/// unless it names a layer). `pos` is its left centre.
pub(crate) fn section(p: &Painter, pos: Pos2, s: &str, c: Color32) -> Rect {
    spaced(p, pos, &s.to_uppercase(), size::SECTION, 1.4, c)
}

/// The power symbol (bypass), centred on `c`.
pub(crate) fn power_icon(p: &Painter, c: Pos2, r: f32, col: Color32) {
    let stroke = Stroke::new(2.0_f32, col);
    let pts: Vec<Pos2> = (0..=28)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let a = std::f32::consts::PI * (-0.5 + 0.22 + 1.56 * i as f32 / 28.0)
                - std::f32::consts::PI;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(Shape::line(pts, stroke));
    p.line_segment([c - vec2(0.0, r + 1.0), c - vec2(0.0, r * 0.15)], stroke);
}

/// A gear (settings), centred on `c`.
pub(crate) fn gear_icon(p: &Painter, c: Pos2, r: f32, col: Color32) {
    let stroke = Stroke::new(1.6_f32, col);
    p.circle_stroke(c, r * 0.42, stroke);
    for i in 0..8 {
        #[allow(clippy::cast_precision_loss)]
        let a = std::f32::consts::PI * 2.0 * i as f32 / 8.0;
        let d = vec2(a.cos(), a.sin());
        p.line_segment([c + d * r * 0.62, c + d * r], Stroke::new(2.4_f32, col));
    }
    p.circle_stroke(c, r * 0.72, stroke);
}

/// A layer's light: lit (with a soft halo) or dark.
pub(crate) fn led(p: &Painter, c: Pos2, on: bool, col: Color32) {
    if on {
        p.circle_filled(c, 7.0, col.gamma_multiply(0.12));
        p.circle_filled(c, 5.0, col.gamma_multiply(0.22));
        p.circle_filled(c, 3.5, col);
    } else {
        p.circle_filled(c, 3.5, color::HOVER);
    }
}

pub(crate) fn well(p: &Painter, r: Rect) {
    fill(p, r, radius::CONTROL, color::WELL);
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
    // A word ("Off", "Jump") stands without its unit.
    let unit = if val.chars().any(|c| c.is_ascii_digit()) {
        unit
    } else {
        ""
    };
    // Inside the inner ring (49/64 of the face), a few points clear of it.
    let room = r.width() * 49.0 / 64.0 - 8.0;
    let max = size.min(r.width() * 0.26);
    let min = 9.0_f32.min(max);
    let layout = |font_size: f32, sep: &str, unit: &str| {
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
                &format!("{sep}{unit}"),
                0.0,
                egui::TextFormat {
                    font_id: value_font((font_size * 0.76).max(7.5)),
                    color: color::TEXT_DIM,
                    ..Default::default()
                },
            );
        }
        p.layout_job(job)
    };
    // Shrink first; at the smallest size drop the space before the
    // unit, then the unit itself: the value never touches the ring.
    let mut font_size = max;
    let galley = loop {
        let g = layout(font_size, " ", unit);
        if g.size().x <= room {
            break g;
        }
        if font_size > min {
            font_size = (font_size - 0.5).max(min);
            continue;
        }
        let g = layout(font_size, "", unit);
        if g.size().x <= room {
            break g;
        }
        break layout(font_size, "", "");
    };
    p.galley(r.center() - galley.size() * 0.5, galley, color::TEXT);
}

/// Button-like box with centred text. `on` lights it in `accent`.
pub(crate) fn pill(p: &Painter, r: Rect, label: &str, on: bool, hover: bool, accent: Color32) {
    let bg = if on {
        accent.gamma_multiply(0.22)
    } else if hover {
        color::HOVER
    } else {
        color::RAISED
    };
    fill(p, r, radius::CONTROL, bg);
    let c = if on { accent } else { color::TEXT_DIM };
    text_fit(
        p,
        r.center(),
        Align2::CENTER_CENTER,
        label,
        size::BODY.min(r.height() * 0.55),
        c,
        r.width() - 12.0,
    );
}
