//! The mixing assistant's panel on the FX page: pick a style, press
//! Listen and sing; it says what it heard and proposes settings, set
//! and heard at once: the proposal follows the style and the three
//! knobs. Compare switches between before and after, Undo puts
//! everything back (the proposal then shows as dots on the knobs, and
//! Apply plays it again). Clarity,
//! Control and Space set how far the tone, the dynamics and the reverb
//! and delay go towards the style.
//!
//! The listening runs here, on the UI thread, from the engine's
//! analysis taps ([`quack_dsp::tap`]): nothing is added to the audio
//! path, so no latency. The thinking is in [`quack_dsp::assist`].

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use quack_dsp::ParamId;
use quack_dsp::assist::{
    Amounts, Analysis, FULL_VOICED_S, Listener, MIN_VOICED_S, PARAMS, Style, describe, propose,
};
use quack_dsp::tap::TAP_LEN;

use crate::controls::{self, KNOB_TRAVEL};
use crate::paint::{knob as paint_knob, knob_value, panel, rounded, text, well};
use crate::theme::{color, radius, size, space};
use crate::{ParamAccess, UiState, View, set_once};

/// Amount a fresh assistant starts at.
const AMOUNT_DEFAULT: f32 = 0.7;
/// A listen gives up after this long without enough singing, seconds.
const LISTEN_MAX_S: f32 = 30.0;

/// What the assistant keeps between frames (not saved with the
/// session: a listen belongs to the moment).
#[derive(Debug, Clone)]
pub(crate) struct AssistUi {
    /// The assistant is shown in place of the EQ's graph.
    pub open: bool,
    pub style: Style,
    /// How far towards the style, 0-1: clarity, control, space.
    pub amounts: [f32; 3],
    /// A listen in progress, and the tap position it has read up to.
    listener: Option<Listener>,
    seen: usize,
    scratch: [Vec<f32>; 2],
    /// Why the last listen gave nothing.
    note: &'static str,
    analysis: Option<Analysis>,
    /// Values from before the first Apply (for Undo and Compare), and
    /// the applied ones while Compare plays the first.
    before: Option<Vec<(ParamId, f64)>>,
    after: Vec<(ParamId, f64)>,
    comparing: bool,
    /// Proposed values not applied yet: the dots on the knobs.
    hints: Vec<(ParamId, f64)>,
    amount_drag: Option<(usize, f32)>,
    /// Style, amounts and tempo of the proposal last put on the
    /// parameters: a change of any is heard at once.
    played: Option<(Style, [f32; 3], u32)>,
}

impl Default for AssistUi {
    fn default() -> Self {
        Self {
            open: false,
            style: Style::default(),
            amounts: [AMOUNT_DEFAULT; 3],
            listener: None,
            seen: 0,
            scratch: [vec![0.0; TAP_LEN], vec![0.0; TAP_LEN]],
            note: "",
            analysis: None,
            before: None,
            after: Vec::new(),
            comparing: false,
            hints: Vec::new(),
            amount_drag: None,
            played: None,
        }
    }
}

impl AssistUi {
    /// The value the assistant proposes for `id`, when it is not the
    /// current one.
    pub fn hint(&self, id: ParamId) -> Option<f64> {
        self.hints.iter().find(|(p, _)| *p == id).map(|(_, v)| *v)
    }

    /// Show a diagnosis (captures and tests).
    pub fn show(&mut self, analysis: Analysis, style: Style, tempo: u32) {
        self.open = true;
        self.style = style;
        self.analysis = Some(analysis);
        // As after Undo: shown, not played.
        self.played = Some((style, self.amounts, tempo));
    }

    fn amounts(&self) -> Amounts {
        let [clarity, control, space] = self.amounts;
        Amounts {
            clarity,
            control,
            space,
        }
    }

    fn stop(&mut self) {
        if let Some(l) = self.listener.take() {
            match l.finish() {
                Some(a) => {
                    self.analysis = Some(a);
                    self.played = None;
                    self.note = "";
                }
                None => self.note = "Not enough singing heard. Try again.",
            }
        }
    }
}

fn snapshot(params: &dyn ParamAccess) -> Vec<(ParamId, f64)> {
    PARAMS
        .iter()
        .map(|id| (*id, params.get_plain(*id)))
        .collect()
}

fn set_all(params: &dyn ParamAccess, values: &[(ParamId, f64)]) {
    for (id, v) in values {
        if (params.get_plain(*id) - v).abs() > 1e-9 {
            set_once(params, *id, *v);
        }
    }
}

/// Once per frame, whatever the page: feed a listen in progress and
/// work out what is still to apply.
pub(crate) fn tick(params: &dyn ParamAccess, st: &mut UiState, view: &View) {
    let UiState { assist: a, eq, .. } = st;
    if let (Some(l), Some(taps)) = (&mut a.listener, &eq.taps) {
        let written = taps.input.written();
        let n = written.wrapping_sub(a.seen).min(TAP_LEN);
        a.seen = written;
        let [x, y] = &mut a.scratch;
        taps.input.read(&mut x[..n]);
        taps.layers.read(&mut y[..n]);
        l.feed(&x[..n], &y[..n], view.pitch_hz);
        if l.voiced_seconds() >= FULL_VOICED_S || l.seconds() >= LISTEN_MAX_S {
            a.stop();
        }
    }
    a.hints.clear();
    if let Some(an) = &a.analysis
        && !a.comparing
        && a.listener.is_none()
    {
        let changes = propose(an, a.style, a.amounts(), view.tempo).changes;
        // A new diagnosis, another style or a turn of Clarity, Control
        // or Space is heard at once. What the user then sets by hand
        // is left alone (and the difference shown as dots).
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let key = (a.style, a.amounts, view.tempo.max(0.0).round() as u32);
        if a.played != Some(key) {
            if a.before.is_none() {
                a.before = Some(snapshot(params));
            }
            set_all(params, &changes);
            a.played = Some(key);
        }
        for (id, v) in changes {
            if (params.get_plain(id) - v).abs() > 1e-6 {
                a.hints.push((id, v));
            }
        }
    }
}

/// The switch between the EQ's graph and the assistant, at the top
/// left of the FX page's left panel.
pub(crate) fn switch(ui: &egui::Ui, st: &mut UiState, r: Rect, accent: Color32) {
    let seg = Rect::from_min_size(pos2(r.left() + space::L, r.top() + 8.0), vec2(150.0, 28.0));
    let mut i = usize::from(st.assist.open);
    controls::tabs(ui, "eq-assist", seg, &["EQ", "Assistant"], &mut i, accent);
    st.assist.open = i == 1;
}

/// A button of the assistant: `true` when clicked.
fn button(
    ui: &egui::Ui,
    salt: &str,
    r: Rect,
    label: &str,
    enabled: bool,
    lit: bool,
    accent: Color32,
) -> bool {
    let resp = ui.interact(r, ui.id().with(("quack-assist", salt)), Sense::click());
    let p = ui.painter();
    let (fill, border, ink) = if lit {
        (
            accent.gamma_multiply(0.20),
            accent.gamma_multiply(0.9),
            accent,
        )
    } else if !enabled {
        (color::PANEL, color::GRID_STRONG, color::BORDER)
    } else if resp.hovered() {
        (color::BORDER, color::BORDER, color::TEXT)
    } else {
        (color::RAISED, color::BORDER, color::TEXT_DIM)
    };
    rounded(p, r, radius::CONTROL, fill, border);
    text(p, r.center(), Align2::CENTER_CENTER, label, size::BODY, ink);
    enabled && resp.clicked()
}

/// One of the three amount knobs (0 Clarity, 1 Control, 2 Space): the
/// same gestures as a parameter's knob, on a value of the editor.
fn amount_knob(ui: &egui::Ui, st: &mut UiState, which: usize, centre: egui::Pos2, accent: Color32) {
    const SIZE: f32 = 48.0;
    let a = &mut st.assist;
    let area = Rect::from_center_size(centre, vec2(SIZE, SIZE));
    let resp = ui.interact(
        area,
        ui.id().with(("quack-assist-amount", which)),
        Sense::click_and_drag(),
    );
    if resp.drag_started() {
        a.amount_drag = Some((which, a.amounts[which]));
    }
    if resp.dragged()
        && let Some((w, from)) = a.amount_drag
        && w == which
        && let (Some(now), Some(origin)) = (
            resp.interact_pointer_pos(),
            ui.input(|i| i.pointer.press_origin()),
        )
    {
        let fine = if ui.input(|i| i.modifiers.shift) {
            0.1
        } else {
            1.0
        };
        let moved = (now.x - origin.x) - (now.y - origin.y);
        a.amounts[which] = (from + moved / KNOB_TRAVEL * fine).clamp(0.0, 1.0);
    }
    if resp.drag_stopped() && a.amount_drag.is_some_and(|(w, _)| w == which) {
        a.amount_drag = None;
    }
    if resp.double_clicked() {
        a.amounts[which] = AMOUNT_DEFAULT;
    } else if resp.hovered() && a.amount_drag.is_none() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            a.amounts[which] = (a.amounts[which] + 0.01 * scroll.signum()).clamp(0.0, 1.0);
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let p = ui.painter();
    paint_knob(p, centre, SIZE, a.amounts[which], accent);
    knob_value(
        p,
        area,
        &format!("{:.0}", a.amounts[which] * 100.0),
        "%",
        size::VALUE,
    );
    text(
        p,
        centre + vec2(0.0, SIZE * 0.5 + 12.0),
        Align2::CENTER_CENTER,
        ["Clarity", "Control", "Space"][which],
        size::CAPTION,
        color::TEXT_DIM,
    );
}

/// The assistant, where the EQ's graph is.
#[allow(clippy::too_many_lines)]
pub(crate) fn assistant_panel(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    view: &View,
    r: Rect,
    accent: Color32,
) {
    panel(ui.painter(), r);
    switch(ui, st, r, accent);
    let left = r.left() + space::L;
    let inner_w = r.width() - 2.0 * space::L;
    let listening = st.assist.listener.is_some();

    // What is heard through the plugin right now.
    let state = if listening {
        "Listening"
    } else if st.assist.comparing {
        "Hearing: before"
    } else if st.assist.before.is_some() {
        "Hearing: after"
    } else {
        ""
    };
    text(
        ui.painter(),
        pos2(r.right() - space::L, r.top() + 22.0),
        Align2::RIGHT_CENTER,
        state,
        size::VALUE,
        if listening { accent } else { color::TEXT_DIM },
    );

    // Styles: two rows of four.
    let gap = 4.0;
    let cw = (inner_w - 3.0 * gap) / 4.0;
    for (i, s) in Style::ALL.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let (col, row) = ((i % 4) as f32, (i / 4) as f32);
        let c = Rect::from_min_size(
            pos2(left + col * (cw + gap), r.top() + 46.0 + row * 30.0),
            vec2(cw, 26.0),
        );
        if button(
            ui,
            s.name(),
            c,
            s.name(),
            true,
            st.assist.style == s,
            accent,
        ) {
            st.assist.style = s;
        }
    }

    // What was heard, in words.
    let box_r = Rect::from_min_size(pos2(left, r.top() + 108.0), vec2(inner_w, 112.0));
    let p = ui.painter();
    well(p, box_r);
    let line = |i: usize, s: &str, c: Color32| {
        #[allow(clippy::cast_precision_loss)]
        let y = box_r.top() + 13.0 + 17.2 * i as f32;
        text(
            p,
            pos2(box_r.left() + space::M, y),
            Align2::LEFT_CENTER,
            s,
            size::CAPTION,
            c,
        );
    };
    if let Some(l) = &st.assist.listener {
        line(0, "Sing a phrase or two, as in the song.", color::TEXT);
        line(
            1,
            &format!(
                "{:.1} s of singing heard, {FULL_VOICED_S:.0} s wanted.",
                l.voiced_seconds()
            ),
            color::TEXT_DIM,
        );
        let bar = Rect::from_min_size(
            pos2(box_r.left() + space::M, box_r.top() + 58.0),
            vec2(box_r.width() - 2.0 * space::M, 6.0),
        );
        rounded(p, bar, 3, color::RAISED, color::RAISED);
        let done = (l.voiced_seconds() / FULL_VOICED_S).clamp(0.0, 1.0);
        if done > 0.0 {
            let fill = Rect::from_min_size(bar.min, vec2(bar.width() * done, bar.height()));
            rounded(p, fill, 3, accent, accent);
        }
        if l.voiced_seconds() >= MIN_VOICED_S {
            line(4, "Stop now for a quick diagnosis.", color::TEXT_FAINT);
        }
    } else if let Some(an) = &st.assist.analysis {
        for (i, s) in describe(an, st.assist.style, st.assist.amounts(), view.tempo)
            .iter()
            .enumerate()
        {
            line(i, s, if i == 0 { color::TEXT } else { color::TEXT_DIM });
        }
    } else {
        line(0, "Pick a style, press Listen and sing.", color::TEXT);
        line(
            1,
            "The assistant says what it hears and proposes",
            color::TEXT_DIM,
        );
        line(2, "settings. You hear them at once.", color::TEXT_DIM);
        line(3, "Compare with before, or Undo it all.", color::TEXT_DIM);
        if !st.assist.note.is_empty() {
            line(4, st.assist.note, color::METER_HOT);
        }
    }

    // Clarity, Control and Space, then Listen, Apply, Compare, Undo.
    let cy = r.bottom() - 58.0;
    for which in 0..3 {
        #[allow(clippy::cast_precision_loss)]
        let x = left + 28.0 + 62.0 * which as f32;
        amount_knob(ui, st, which, pos2(x, cy), accent);
    }
    let bw = 95.0;
    let bx = r.right() - space::L - 2.0 * bw - 6.0;
    #[allow(clippy::cast_precision_loss)]
    let slot = |i: usize| {
        Rect::from_min_size(
            pos2(
                bx + (bw + 6.0) * (i % 2) as f32,
                cy - 30.0 + 34.0 * (i / 2) as f32,
            ),
            vec2(bw, 28.0),
        )
    };
    let a = &mut st.assist;
    let has_taps = st.eq.taps.is_some();
    if button(
        ui,
        "listen",
        slot(0),
        if listening { "Stop" } else { "Listen" },
        has_taps,
        listening,
        accent,
    ) {
        if listening {
            a.stop();
        } else if let Some(taps) = &st.eq.taps {
            let sr = if view.sample_rate > 0.0 {
                view.sample_rate
            } else {
                48_000.0
            };
            a.listener = Some(Listener::new(sr));
            a.seen = taps.input.written();
            a.note = "";
        }
    }
    let can_apply = !listening && !a.comparing && !a.hints.is_empty();
    if button(ui, "apply", slot(1), "Apply", can_apply, false, accent) {
        if a.before.is_none() {
            a.before = Some(snapshot(params));
        }
        set_all(params, &a.hints);
        a.hints.clear();
    }
    let applied = a.before.is_some() && !listening;
    if button(
        ui,
        "compare",
        slot(2),
        "Compare",
        applied,
        a.comparing,
        accent,
    ) && let Some(before) = &a.before
    {
        if a.comparing {
            set_all(params, &a.after);
        } else {
            a.after = snapshot(params);
            set_all(params, before);
        }
        a.comparing = !a.comparing;
    }
    if button(ui, "undo", slot(3), "Undo", applied, false, accent)
        && let Some(before) = a.before.take()
    {
        set_all(params, &before);
        a.comparing = false;
    }
}
