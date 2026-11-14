//! The mixing assistant's panel on the FX page: pick a style, press
//! Listen and sing; it says what it heard and proposes settings, set
//! and heard at once: the proposal follows the style and the three
//! knobs. Compare switches between before and after, Undo puts
//! everything back (the proposal then shows as dots on the knobs, and
//! Apply plays it again). Clarity,
//! Control and Space set how far the tone, the dynamics and the reverb
//! and delay go towards the style.
//!
//! A second Listen, with the proposal playing, checks it at the output
//! and corrects the EQ for what the exciter and the reverb add. When the
//! side-chain is free (not the voice, not the vocoder's carrier), what
//! comes by it is taken for the mix and the voice is shaped to cut
//! through it.
//!
//! The listening runs here, on the UI thread, from the engine's
//! analysis taps ([`quack_dsp::tap`]): nothing is added to the audio
//! path, so no latency. The thinking is in [`quack_dsp::assist`].

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use quack_dsp::ParamId;
use quack_dsp::assist::{
    Amounts, Analysis, BANDS, FULL_VOICED_S, Heard, Listener, MIN_VOICED_S, PARAMS, Style,
    describe, eq_in_force, is_effect, propose,
};
use quack_dsp::tap::TAP_LEN;
use quack_dsp::vocoder::Carrier;

use crate::controls::{self, KNOB_TRAVEL};
use crate::paint::{knob as paint_knob, knob_value, panel, rounded, text, well};
use crate::theme::{color, radius, size, space};
use crate::{ParamAccess, UiState, View, set_once};

/// Amount a fresh assistant starts at.
const AMOUNT_DEFAULT: f32 = 0.7;
/// A listen gives up after this long without enough singing, seconds.
const LISTEN_MAX_S: f32 = 30.0;
/// Singing the check at the output asks for, and how long the
/// proposal is left to settle before it starts, seconds.
const CHECK_VOICED_S: f32 = 4.0;
const SETTLE_S: f64 = 0.7;

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
    scratch: [Vec<f32>; 4],
    /// The listen in progress is the check at the output, which follows
    /// the first one by itself; `check_from` is the tap position it
    /// waits for (the proposal has to be playing first).
    checking: bool,
    check_from: Option<usize>,
    /// What the check changed, in a few words.
    verdict: String,
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
    /// Style and amounts the output was last checked with: once they
    /// move, the check is stale and Check asks for another.
    checked_with: Option<(Style, [f32; 3])>,
    /// The user let this listen set the effects (reverb, delay, sends):
    /// asked before each one, `asking` while the question is up.
    effects: bool,
    asking: bool,
    /// What the next proposal may write (after a check: not the
    /// effects).
    scope: Scope,
}

/// Which of the proposed values are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    All,
    /// Everything but the effects.
    Rest,
    /// The effects only.
    Effects,
}

impl Scope {
    fn takes(self, id: ParamId) -> bool {
        match self {
            Self::All => true,
            Self::Rest => !is_effect(id),
            Self::Effects => is_effect(id),
        }
    }
}

impl Default for AssistUi {
    fn default() -> Self {
        Self {
            open: false,
            style: Style::default(),
            amounts: [AMOUNT_DEFAULT; 3],
            listener: None,
            seen: 0,
            scratch: std::array::from_fn(|_| vec![0.0; TAP_LEN]),
            checking: false,
            check_from: None,
            verdict: String::new(),
            note: "",
            analysis: None,
            before: None,
            after: Vec::new(),
            comparing: false,
            hints: Vec::new(),
            amount_drag: None,
            played: None,
            checked_with: None,
            effects: true,
            asking: false,
            scope: Scope::All,
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
        self.checked_with = Some((style, self.amounts));
    }

    /// Put the question a listen starts with (captures and tests),
    /// as answered `effects` the last time.
    pub fn ask(&mut self, effects: bool) {
        self.asking = true;
        self.effects = effects;
    }

    fn amounts(&self) -> Amounts {
        let [clarity, control, space] = self.amounts;
        Amounts {
            clarity,
            control,
            // At 0 the proposal leaves the effects out.
            space: if self.effects { space } else { 0.0 },
        }
    }

    /// The style or an amount moved since the output was checked.
    fn stale(&self) -> bool {
        self.analysis.as_ref().is_some_and(|a| a.checked)
            && self.checked_with != Some((self.style, self.amounts))
    }

    /// Seconds of singing the listen in progress asks for.
    fn wanted(&self) -> f32 {
        if self.checking {
            CHECK_VOICED_S
        } else {
            FULL_VOICED_S
        }
    }

    /// End the listen; `eq_db` is the EQ in force while it ran, `tempo`
    /// the host's. After a first listen the check at the output
    /// follows by itself (`settle` tap samples later); the check's own
    /// end says what it changed.
    /// `deessing`: the output de-esser was at work, so the sibilants
    /// measured there are no lower bound on what it has to tame.
    fn stop(&mut self, eq_db: &[f32; BANDS], tempo: f32, settle: usize, deessing: bool) {
        let Some(l) = self.listener.take() else {
            return;
        };
        let checking = std::mem::take(&mut self.checking);
        match (l.finish(eq_db), checking) {
            (Some(mut a), true) => {
                a.checked = true;
                if let (true, Some(first)) = (deessing, &self.analysis)
                    && first.checked
                {
                    a.out_sibilance = match (a.out_sibilance, first.out_sibilance) {
                        (Some(x), Some(y)) => Some(x.max(y)),
                        (x, y) => x.or(y),
                    };
                }
                self.checked_with = Some((self.style, self.amounts));
                self.verdict = match &self.analysis {
                    Some(first) => verdict(first, &a, self.style, self.amounts(), tempo),
                    None => String::new(),
                };
                self.analysis = Some(a);
                self.played = None;
                // What the user set on the effects since stays.
                self.scope = Scope::Rest;
            }
            // Not enough singing for the check: the first diagnosis
            // stands.
            (None, true) => "Check skipped".clone_into(&mut self.verdict),
            (Some(a), false) => {
                self.analysis = Some(a);
                self.played = None;
                self.scope = Scope::All;
                self.note = "";
                self.verdict.clear();
                self.checked_with = None;
                self.check_from = Some(self.seen.wrapping_add(settle));
            }
            (None, false) => self.note = "Not enough singing heard. Try again.",
        }
    }
}

/// What the check at the output changed in the EQ and the output
/// de-esser, in a few words.
fn verdict(
    first: &Analysis,
    checked: &Analysis,
    style: Style,
    amounts: Amounts,
    tempo: f32,
) -> String {
    let before = propose(first, style, amounts, tempo);
    let after = propose(checked, style, amounts, tempo);
    let mut parts: Vec<String> = Vec::new();
    for (id, name) in [
        (quack_dsp::params::EQ_BELL_PARAMS[0][1], "lows"),
        (quack_dsp::params::EQ_BELL_PARAMS[2][1], "presence"),
        (ParamId::EqHighShelfGain, "air"),
    ] {
        let d = after.get(id).unwrap_or(0.0) - before.get(id).unwrap_or(0.0);
        if d.abs() >= 0.5 {
            parts.push(format!("{name} {d:+.1}"));
        }
    }
    if after.get(ParamId::OutDeEss).unwrap_or(0.0) >= 1.0 {
        parts.push("de-ess".to_owned());
    }
    if parts.is_empty() {
        "Checked: all good".to_owned()
    } else {
        parts.truncate(2);
        format!("Checked: {}", parts.join(", "))
    }
}

/// The output EQ as it is set now, dB per analysis band.
fn eq_now(params: &dyn ParamAccess, view: &View) -> [f32; BANDS] {
    #[allow(clippy::cast_possible_truncation)]
    let sr = if view.sample_rate > 0.0 {
        view.sample_rate as f32
    } else {
        48_000.0
    };
    eq_in_force(&|id| params.get_plain(id), sr)
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
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let settle = (view.sample_rate.max(8000.0) * SETTLE_S) as usize;
    // The check at the output starts by itself once the proposal of
    // the first listen has been playing for a moment.
    if let (Some(from), Some(taps), None) = (a.check_from, &eq.taps, &a.listener)
        && taps.input.written().wrapping_sub(from) < usize::MAX / 2
    {
        a.check_from = None;
        a.listener = Some(Listener::new(view.sample_rate.max(8000.0)));
        a.seen = taps.input.written();
        a.checking = true;
    }
    if let (Some(l), Some(taps)) = (&mut a.listener, &eq.taps) {
        let written = taps.input.written();
        let n = written.wrapping_sub(a.seen).min(TAP_LEN);
        a.seen = written;
        let [x, y, z, w] = &mut a.scratch;
        taps.input.read(&mut x[..n]);
        taps.layers.read(&mut y[..n]);
        taps.output.read(&mut z[..n]);
        // The mix comes from Quackapella Link when one sends it, else
        // from the side-chain unless that brings the voice or the
        // vocoder's carrier.
        let linked = taps.linked.load(std::sync::atomic::Ordering::Relaxed);
        if linked {
            taps.link.read(&mut w[..n]);
        } else {
            taps.side.read(&mut w[..n]);
        }
        let taken = !linked
            && (view.link[1]
                || params.get_plain(ParamId::VocoderSwap) >= 0.5
                || (params.get_plain(ParamId::VocoderOn) >= 0.5
                    && Carrier::from_index(params.get_plain(ParamId::Carrier))
                        == Carrier::Sidechain));
        l.feed(
            &Heard {
                input: &x[..n],
                layers: &y[..n],
                output: &z[..n],
                side: (!taken).then_some(&w[..n]),
            },
            view.pitch_hz,
        );
        let wanted = if a.checking {
            CHECK_VOICED_S
        } else {
            FULL_VOICED_S
        };
        if l.voiced_seconds() >= wanted || l.seconds() >= LISTEN_MAX_S {
            let deessing = params.get_plain(ParamId::OutDeEss) >= 1.0;
            a.stop(&eq_now(params, view), view.tempo, settle, deessing);
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
        let dose = a.amounts();
        let key = (
            a.style,
            [dose.clarity, dose.control, dose.space],
            view.tempo.max(0.0).round() as u32,
        );
        if a.played != Some(key) {
            if a.before.is_none() {
                a.before = Some(snapshot(params));
            }
            // Only what moved is written: Clarity and Control leave the
            // effects as the user set them, Space and the tempo leave
            // the rest. A new style or a new listen writes it all.
            let scope = match a.played {
                None => a.scope,
                Some((style, _, _)) if style != key.0 => Scope::All,
                Some((_, was, tempo)) => {
                    let effects = was[2] != key.1[2] || tempo != key.2;
                    let rest = was[0] != key.1[0] || was[1] != key.1[1];
                    match (effects, rest) {
                        (true, false) => Scope::Effects,
                        (false, true) => Scope::Rest,
                        _ => Scope::All,
                    }
                }
            };
            let write: Vec<(ParamId, f64)> = changes
                .iter()
                .copied()
                .filter(|(id, _)| scope.takes(*id))
                .collect();
            set_all(params, &write);
            a.scope = Scope::All;
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
        // Turning Space is asking for the effects.
        a.effects |= which == 2;
    }
    if resp.drag_stopped() && a.amount_drag.is_some_and(|(w, _)| w == which) {
        a.amount_drag = None;
    }
    if resp.double_clicked() {
        a.amounts[which] = AMOUNT_DEFAULT;
        a.effects |= which == 2;
    } else if resp.hovered() && a.amount_drag.is_none() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            a.amounts[which] = (a.amounts[which] + 0.01 * scroll.signum()).clamp(0.0, 1.0);
            a.effects |= which == 2;
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let p = ui.painter();
    // Space at rest while the effects are the user's own.
    let kept = which == 2 && !a.effects;
    paint_knob(
        p,
        centre,
        SIZE,
        a.amounts[which],
        if kept {
            accent.gamma_multiply(0.35)
        } else {
            accent
        },
    );
    if kept {
        knob_value(p, area, "Kept", "", size::VALUE);
    } else {
        knob_value(
            p,
            area,
            &format!("{:.0}", a.amounts[which] * 100.0),
            "%",
            size::VALUE,
        );
    }
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
    let state = if st.assist.checking {
        "Checking the output"
    } else if listening {
        "Listening"
    } else if st.assist.comparing {
        "Hearing: before"
    } else if st.assist.stale() {
        "Changed: check again"
    } else if !st.assist.verdict.is_empty() {
        st.assist.verdict.as_str()
    } else if st.assist.before.is_some() {
        "Hearing: after"
    } else {
        ""
    };
    // The switch takes the left 150: the state keeps to what is left.
    crate::paint::text_fit(
        ui.painter(),
        pos2(r.right() - space::L, r.top() + 22.0),
        Align2::RIGHT_CENTER,
        state,
        size::CAPTION,
        if listening { accent } else { color::TEXT_DIM },
        inner_w - 150.0 - space::M,
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
        crate::paint::text_fit(
            p,
            pos2(box_r.left() + space::M, y),
            Align2::LEFT_CENTER,
            s,
            size::CAPTION,
            c,
            box_r.width() - 2.0 * space::M,
        );
    };
    if st.assist.asking {
        line(0, "Should this listen set the effects too?", color::TEXT);
        line(
            1,
            "Reverb, delay and the layers' sends. Keep mine",
            color::TEXT_DIM,
        );
        line(2, "leaves them exactly as they are.", color::TEXT_DIM);
        let bw = (box_r.width() - 3.0 * space::M) / 2.0;
        let slot = |i: f32| {
            Rect::from_min_size(
                pos2(
                    box_r.left() + space::M + (bw + space::M) * i,
                    box_r.bottom() - 28.0 - space::M,
                ),
                vec2(bw, 28.0),
            )
        };
        // The last answer is the lit one.
        let was = st.assist.effects;
        let mut answer = None;
        if button(ui, "fx-keep", slot(0.0), "Keep mine", true, !was, accent) {
            answer = Some(false);
        }
        if button(
            ui,
            "fx-set",
            slot(1.0),
            "Set the effects",
            true,
            was,
            accent,
        ) {
            answer = Some(true);
        }
        if let (Some(effects), Some(taps)) = (answer, &st.eq.taps) {
            let a = &mut st.assist;
            a.effects = effects;
            a.asking = false;
            a.listener = Some(Listener::new(view.sample_rate.max(8000.0)));
            a.seen = taps.input.written();
            a.note = "";
            a.checking = false;
            a.check_from = None;
            a.verdict.clear();
        }
    } else if let Some(l) = &st.assist.listener {
        let wanted = st.assist.wanted();
        if st.assist.checking {
            line(0, "Checking the result at the output.", color::TEXT);
            line(
                2,
                "Keep singing: the EQ and the output are refined",
                color::TEXT_DIM,
            );
            line(
                3,
                "for what the exciter and the reverb add.",
                color::TEXT_DIM,
            );
        } else {
            line(0, "Sing a phrase or two, as in the song.", color::TEXT);
        }
        line(
            1,
            &format!(
                "{:.1} s of singing heard, {wanted:.0} s wanted.",
                l.voiced_seconds()
            ),
            color::TEXT_DIM,
        );
        let bar = Rect::from_min_size(
            pos2(box_r.left() + space::M, box_r.bottom() - 34.0),
            vec2(box_r.width() - 2.0 * space::M, 6.0),
        );
        rounded(p, bar, 3, color::RAISED, color::RAISED);
        let done = (l.voiced_seconds() / wanted).clamp(0.0, 1.0);
        if done > 0.0 {
            let fill = Rect::from_min_size(bar.min, vec2(bar.width() * done, bar.height()));
            rounded(p, fill, 3, accent, accent);
        }
        if l.voiced_seconds() >= MIN_VOICED_S && !st.assist.checking {
            line(2, "Stop now for a quick diagnosis.", color::TEXT_FAINT);
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
        line(
            4,
            "Quackapella Link on the mix: it is heard too.",
            color::TEXT_DIM,
        );
        if !st.assist.note.is_empty() {
            line(5, st.assist.note, color::METER_HOT);
        }
    }

    // Clarity, Control and Space, then Listen, Check and Apply over
    // Compare and Undo.
    let cy = r.bottom() - 58.0;
    for which in 0..3 {
        #[allow(clippy::cast_precision_loss)]
        let x = left + 28.0 + 62.0 * which as f32;
        amount_knob(ui, st, which, pos2(x, cy), accent);
    }
    let bw = 95.0;
    let bx = r.right() - space::L - 2.0 * bw - 6.0;
    #[allow(clippy::cast_precision_loss)]
    let slot =
        |i: usize| Rect::from_min_size(pos2(bx + (bw + 6.0) * i as f32, cy + 4.0), vec2(bw, 28.0));
    let tw = (2.0 * bw - 6.0) / 3.0;
    #[allow(clippy::cast_precision_loss)]
    let top =
        |i: usize| Rect::from_min_size(pos2(bx + (tw + 6.0) * i as f32, cy - 30.0), vec2(tw, 28.0));
    let a = &mut st.assist;
    let has_taps = st.eq.taps.is_some();
    if button(
        ui,
        "listen",
        top(0),
        if listening {
            "Stop"
        } else if a.asking {
            "Cancel"
        } else {
            "Listen"
        },
        has_taps,
        listening || a.asking,
        accent,
    ) {
        if a.asking {
            a.asking = false;
        } else if listening {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let settle = (view.sample_rate.max(8000.0) * SETTLE_S) as usize;
            let deessing = params.get_plain(ParamId::OutDeEss) >= 1.0;
            a.stop(&eq_now(params, view), view.tempo, settle, deessing);
        } else {
            // First the question: the effects too, or not?
            a.asking = true;
            a.check_from = None;
        }
    }
    // Another check at the output: lit when the style or an amount
    // moved since the last one, or when it was skipped.
    let can_check = has_taps && !listening && !a.asking && !a.comparing && a.analysis.is_some();
    let due = can_check && (a.stale() || a.analysis.as_ref().is_some_and(|an| !an.checked));
    if button(ui, "check", top(1), "Check", can_check, due, accent)
        && let Some(taps) = &st.eq.taps
    {
        a.check_from = Some(taps.input.written());
        a.verdict.clear();
    }
    let can_apply = !listening && !a.asking && !a.comparing && !a.hints.is_empty();
    if button(ui, "apply", top(2), "Apply", can_apply, false, accent) {
        if a.before.is_none() {
            a.before = Some(snapshot(params));
        }
        set_all(params, &a.hints);
        a.hints.clear();
    }
    let applied = a.before.is_some() && !listening && !a.asking;
    if button(
        ui,
        "compare",
        slot(0),
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
    if button(ui, "undo", slot(1), "Undo", applied, false, accent)
        && let Some(before) = a.before.take()
    {
        set_all(params, &before);
        a.comparing = false;
    }
}
