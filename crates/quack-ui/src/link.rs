//! The editor of Quackapella Link, the companion plugin: what the track
//! is sent as, how loud, and whether Quackapella receives it. Same
//! theme, same knob as the main editor.

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};

use crate::brand;
use crate::controls::KNOB_TRAVEL;
use crate::paint::{knob, knob_value, panel, rounded, spaced, text, text_fit, well};
use crate::theme::{color, radius, size, space};

/// The window, in points.
pub const LINK_W: u32 = 460;
pub const LINK_H: u32 = 302;

const LEVEL_MIN: f64 = -24.0;
const LEVEL_MAX: f64 = 12.0;
/// Bottom of the meter, dBFS.
const METER_FLOOR: f32 = -60.0;

/// The two host parameters the editor moves.
pub trait LinkAccess {
    /// 0 Mix, 1 Carrier.
    fn role(&self) -> usize;
    fn set_role(&self, role: usize);
    fn level_db(&self) -> f64;
    fn begin_level(&self);
    fn set_level_db(&self, db: f64);
    fn end_level(&self);
}

/// Where the link stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkState {
    /// Bypassed, or no audio has run yet.
    #[default]
    Off,
    /// On the wire; `heard` when a Quackapella receives it.
    Sending { heard: bool },
    /// Another Link already sends as this role.
    Busy,
    /// The system gave no shared memory.
    Unavailable,
}

/// What the audio thread shows, once per frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinkView {
    /// Peaks sent since the last frame, linear, left and right.
    pub peak: [f32; 2],
    pub state: LinkState,
    /// Blocks processed so far: stands still when the host sleeps.
    pub blocks: u64,
}

/// What the editor keeps between frames.
#[derive(Debug)]
pub struct LinkUi {
    /// Level when the drag started, normalized.
    drag: Option<f64>,
    /// The meter's bars and their held peaks, dBFS.
    bar: [f32; 2],
    hold: [(f32, f64); 2],
    /// The block count last seen, and when it last moved.
    blocks: u64,
    moved_at: f64,
}

impl Default for LinkUi {
    fn default() -> Self {
        Self {
            drag: None,
            bar: [METER_FLOOR; 2],
            hold: [(METER_FLOOR, 0.0); 2],
            blocks: 0,
            moved_at: f64::NEG_INFINITY,
        }
    }
}

impl LinkUi {
    /// The level knob is held (its edit is open).
    #[must_use]
    pub const fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

fn accent_of(role: usize) -> Color32 {
    if role == 0 { color::FX } else { color::VOCODER }
}

/// The whole editor.
#[allow(clippy::too_many_lines)]
pub fn draw(ui: &mut egui::Ui, access: &dyn LinkAccess, view: &LinkView, st: &mut LinkUi) {
    let full = ui.max_rect();
    let p = ui.painter().clone();
    p.rect_filled(full, 0.0, color::BG);
    let inner = full.shrink(space::XL);
    let role = access.role().min(1);
    let accent = accent_of(role);
    let now = ui.input(|i| i.time);
    let dt = ui.input(|i| i.stable_dt).min(0.1);

    // The host runs us (a stopped host sleeps silent tracks).
    if view.blocks != st.blocks {
        st.blocks = view.blocks;
        st.moved_at = now;
    }
    let awake = now - st.moved_at < 0.5;

    // Header: the family's wordmark, then this plugin's name.
    let head_y = inner.top() + 12.0;
    let mark = spaced(
        &p,
        pos2(inner.left(), head_y),
        "QUACKAPELLA",
        size::TITLE,
        3.0,
        color::TEXT,
    );
    spaced(
        &p,
        pos2(mark.right() + 10.0, head_y),
        "LINK",
        size::TITLE,
        3.0,
        accent,
    );
    p.line_segment(
        [
            pos2(inner.left(), head_y + 17.0),
            pos2(inner.left() + 36.0, head_y + 17.0),
        ],
        egui::Stroke::new(2.0_f32, color::QUACK),
    );

    // One panel: role and level on top, what it does, then the meter.
    let body = Rect::from_min_max(
        pos2(inner.left(), head_y + 34.0),
        pos2(inner.right(), inner.bottom() - 28.0),
    );
    panel(&p, body);
    let left = body.left() + space::L;
    let right = body.right() - space::L;

    // Level, on the right: the knob with its value inside.
    let knob_d = 62.0;
    let centre = pos2(
        right - knob_d * 0.5 - 6.0,
        body.top() + space::L + knob_d * 0.5,
    );
    level_knob(ui, access, st, centre, knob_d, accent);
    let text_right = centre.x - knob_d * 0.5 - space::L;

    // Role, on the left.
    text(
        &p,
        pos2(left, body.top() + space::L + 7.0),
        Align2::LEFT_CENTER,
        "Send this track as",
        size::LABEL,
        color::TEXT_DIM,
    );
    let seg = Rect::from_min_size(
        pos2(left, body.top() + space::L + 22.0),
        vec2((text_right - left).min(230.0), 32.0),
    );
    rounded(&p, seg, radius::CONTROL, color::WELL, color::BORDER);
    let labels = ["Mix", "Carrier"];
    let w = seg.width() / 2.0;
    for (i, label) in labels.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let r = Rect::from_min_size(
            pos2(seg.left() + w * i as f32, seg.top()),
            vec2(w, seg.height()),
        );
        let resp = ui.interact(r, ui.id().with(("quack-link-role", i)), Sense::click());
        if resp.clicked() && i != role {
            access.set_role(i);
        }
        let a = accent_of(i);
        if i == role {
            rounded(
                &p,
                r.shrink(2.0),
                radius::CONTROL - 1,
                a.gamma_multiply(0.22),
                a.gamma_multiply(0.8),
            );
        } else if resp.hovered() {
            rounded(
                &p,
                r.shrink(2.0),
                radius::CONTROL - 1,
                color::RAISED,
                color::RAISED,
            );
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        text(
            &p,
            r.center(),
            Align2::CENTER_CENTER,
            label,
            size::BODY,
            if i == role { a } else { color::TEXT_DIM },
        );
    }

    // What the role does, in two lines.
    let lines = if role == 0 {
        [
            "The mixing assistant hears this track",
            "as the rest of the song.",
        ]
    } else {
        [
            "This track plays the vocoder:",
            "in Quackapella, pick the Sidechain carrier.",
        ]
    };
    let help_y = seg.bottom() + space::L + 2.0;
    for (i, l) in lines.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let y = help_y + 18.0 * i as f32;
        text_fit(
            &p,
            pos2(left, y),
            Align2::LEFT_CENTER,
            l,
            size::CAPTION,
            if i == 0 { color::TEXT } else { color::TEXT_DIM },
            right - left,
        );
    }

    // Meter: what goes on the wire, after the level.
    let meter_top = help_y + 18.0 + space::L + 2.0;
    let sending = awake && matches!(view.state, LinkState::Sending { .. });
    let bar_left = left + 18.0;
    let bar_right = right - 46.0;
    let x_of = |db: f32| {
        let t = ((db - METER_FLOOR) / -METER_FLOOR).clamp(0.0, 1.0);
        bar_left + (bar_right - bar_left) * t
    };
    for ch in 0..2 {
        let peak_db = if sending && view.peak[ch] > 0.0 {
            20.0 * view.peak[ch].log10()
        } else {
            METER_FLOOR
        };
        // Up at once, down at 30 dB a second; the peak holds a second.
        st.bar[ch] = peak_db.max(st.bar[ch] - 30.0 * dt).max(METER_FLOOR);
        if peak_db >= st.hold[ch].0 || now - st.hold[ch].1 > 1.0 {
            st.hold[ch] = (peak_db.max(st.bar[ch]), now);
        }
        #[allow(clippy::cast_precision_loss)]
        let y = meter_top + 14.0 * ch as f32;
        text(
            &p,
            pos2(left, y + 4.0),
            Align2::LEFT_CENTER,
            if ch == 0 { "L" } else { "R" },
            size::CAPTION - 1.5,
            color::TEXT_FAINT,
        );
        let track = Rect::from_min_max(pos2(bar_left, y), pos2(bar_right, y + 8.0));
        well(&p, track);
        let lit = st.bar[ch];
        if lit > METER_FLOOR + 0.5 {
            // Three zones, as on Quackapella's meters.
            for (from, to, c) in [
                (METER_FLOOR, -6.0, color::METER_OK),
                (-6.0, -1.0, color::METER_HOT),
                (-1.0, 0.0, color::METER_OVER),
            ] {
                if lit > from {
                    let r = Rect::from_min_max(
                        pos2(x_of(from).max(track.left() + 1.0), track.top() + 1.0),
                        pos2(
                            x_of(lit.min(to)).min(track.right() - 1.0),
                            track.bottom() - 1.0,
                        ),
                    );
                    p.rect_filled(r, 1.0, c);
                }
            }
        }
        let held = st.hold[ch].0;
        if held > METER_FLOOR + 0.5 {
            let x = x_of(held).min(track.right() - 1.5);
            p.line_segment(
                [pos2(x, track.top() + 1.0), pos2(x, track.bottom() - 1.0)],
                egui::Stroke::new(1.5_f32, color::TEXT),
            );
        }
    }
    // The loudest of the two held peaks, in figures.
    let held = st.hold[0].0.max(st.hold[1].0);
    let reading = if held > METER_FLOOR + 0.5 {
        format!("{held:.1}")
    } else {
        "-inf".to_owned()
    };
    crate::paint::value(
        &p,
        pos2(right, meter_top + 11.0),
        Align2::RIGHT_CENTER,
        &reading,
        size::CAPTION - 0.5,
        if held > -1.0 {
            color::METER_OVER
        } else {
            color::TEXT_DIM
        },
    );
    for db in [-48.0_f32, -24.0, -12.0, -6.0, 0.0] {
        let x = x_of(db);
        p.line_segment(
            [pos2(x, meter_top + 24.0), pos2(x, meter_top + 27.0)],
            egui::Stroke::new(1.0_f32, color::GRID_STRONG),
        );
        text(
            &p,
            pos2(x, meter_top + 35.0),
            Align2::CENTER_CENTER,
            &format!("{db:.0}"),
            size::CAPTION - 2.5,
            color::TEXT_FAINT,
        );
    }

    // Footer: the link's state, then the house mark.
    let foot_y = inner.bottom() - 7.0;
    let (dot, words) = match view.state {
        _ if !awake => (color::TEXT_FAINT, "Waiting for the host to run this track"),
        LinkState::Sending { heard: true } => (color::NATURAL, "Quackapella is receiving"),
        LinkState::Sending { heard: false } => {
            (color::METER_HOT, "No Quackapella is listening yet")
        }
        LinkState::Busy if role == 0 => (color::METER_OVER, "Another Link already sends the mix"),
        LinkState::Busy => (color::METER_OVER, "Another Link already sends the carrier"),
        LinkState::Unavailable => (color::METER_OVER, "The system refused the link"),
        LinkState::Off => (color::TEXT_FAINT, "Bypassed: nothing is sent"),
    };
    p.circle_filled(pos2(inner.left() + 4.0, foot_y), 4.0, dot);
    let mark_h = 12.0;
    let mark_w = brand::Brand::DuckDsp.width_for(mark_h);
    text_fit(
        &p,
        pos2(inner.left() + 16.0, foot_y),
        Align2::LEFT_CENTER,
        words,
        size::CAPTION,
        color::TEXT_DIM,
        inner.width() - 16.0 - mark_w - space::L,
    );
    let mark = Rect::from_min_size(
        pos2(inner.right() - mark_w, foot_y - mark_h * 0.5),
        vec2(mark_w, mark_h),
    );
    brand::paint(ui, brand::Brand::DuckDsp, mark, color::TEXT_FAINT);
}

/// The send level: drag, shift for fine, wheel, double-click for 0 dB.
fn level_knob(
    ui: &egui::Ui,
    access: &dyn LinkAccess,
    st: &mut LinkUi,
    centre: egui::Pos2,
    d: f32,
    accent: Color32,
) {
    let norm = |db: f64| (db - LEVEL_MIN) / (LEVEL_MAX - LEVEL_MIN);
    let denorm = |n: f64| {
        let db = LEVEL_MIN + n.clamp(0.0, 1.0) * (LEVEL_MAX - LEVEL_MIN);
        (db * 10.0).round() / 10.0
    };
    let area = Rect::from_center_size(centre, vec2(d, d));
    let resp = ui.interact(
        area,
        ui.id().with("quack-link-level"),
        Sense::click_and_drag(),
    );
    let db = access.level_db();
    if resp.drag_started() {
        access.begin_level();
        st.drag = Some(norm(db));
    }
    if resp.dragged()
        && let Some(n0) = st.drag
        && let (Some(at), Some(from)) = (
            resp.interact_pointer_pos(),
            ui.input(|i| i.pointer.press_origin()),
        )
    {
        let fine = if ui.input(|i| i.modifiers.shift) {
            0.1
        } else {
            1.0
        };
        let moved = (at.x - from.x) - (at.y - from.y);
        access.set_level_db(denorm(n0 + f64::from(moved / KNOB_TRAVEL) * fine));
    }
    if resp.drag_stopped() && st.drag.take().is_some() {
        access.end_level();
    }
    let once = |db: f64| {
        access.begin_level();
        access.set_level_db(db);
        access.end_level();
    };
    if resp.double_clicked() {
        once(0.0);
    } else if resp.hovered() && st.drag.is_none() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            once(denorm(norm(db) + f64::from(scroll.signum()) * 0.01));
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let p = ui.painter();
    let db = access.level_db();
    #[allow(clippy::cast_possible_truncation)]
    knob(p, centre, d, norm(db) as f32, accent);
    knob_value(p, area, &format!("{db:+.1}"), "dB", size::VALUE);
    text(
        p,
        centre + vec2(0.0, d * 0.5 + 12.0),
        Align2::CENTER_CENTER,
        "Send level",
        size::CAPTION,
        color::TEXT_DIM,
    );
}
