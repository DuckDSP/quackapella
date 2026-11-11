//! Quackapella's editor, in the DuckDSP look shared with Sonder
//! (`theme`, `brand`): dark flat surfaces, Inter for labels, JetBrains
//! Mono for values, one accent per mode.
//!
//! The editor knows nothing of the plugin framework: it reads and
//! writes parameters through [`ParamAccess`] (plain values) and reads
//! what the audio thread publishes through [`View`]. That keeps it
//! testable headless and lets the plugin adapter change without
//! touching the UI.
//!
//! Step 1 shows the mode, the level controls, bypass, the MIDI notes
//! received and the latency; step 2 adds the detected pitch (note,
//! cents, Hz, confidence) as a tuner strip; step 3 the Voice panel
//! (transposition, fine tune, formant shift); step 4 the Tune panel
//! (key, scale, note switches, MIDI targets, retune, humanize, vibrato)
//! and the pitch graph, which scrolls the sung pitch, the target notes
//! and the corrected pitch. Panels for harmonies and vocoder come with
//! their DSP steps (roadmap step 8 for presets, macros and MIDI learn).

mod brand;
mod controls;
mod paint;
pub mod theme;

use egui::{Align2, Color32, Rect, pos2, vec2};
use quack_dsp::autotune::{Scale, Source, scale_mask};
use quack_dsp::creative::{GlitchMode, GlitchRate, glitch_step};
use quack_dsp::harmony::{HarmonyMode, interval_short, interval_steps};
use quack_dsp::latency::VoiceRange;
use quack_dsp::params::NOTE_PARAMS;
use quack_dsp::{ParamId, VoiceMode};

use paint::{panel, spaced, text, value, well};
use theme::{color, size, space};

/// Editor size in points (fixed for now; zoom comes with step 8, with
/// the AUv2 resize patch already in `vendor/`).
pub const BASE_W: u32 = 1040;
pub const BASE_H: u32 = 720;

/// Parameter access in plain values, implemented over the host by the
/// plugin and by a plain array in tests.
pub trait ParamAccess {
    fn get_plain(&self, id: ParamId) -> f64;
    fn begin_edit(&self, id: ParamId);
    fn set_plain(&self, id: ParamId, plain: f64);
    fn end_edit(&self, id: ParamId);
}

/// One edit on its own (click, double-click, wheel step).
pub(crate) fn set_once(params: &dyn ParamAccess, id: ParamId, plain: f64) {
    params.begin_edit(id);
    params.set_plain(id, plain);
    params.end_edit(id);
}

/// What the audio thread published for this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct View {
    /// Held MIDI notes, bit n = note n.
    pub held_notes: u128,
    /// Reported latency in samples, and the rate it was computed at.
    pub latency_samples: u32,
    pub sample_rate: f64,
    /// Detected pitch in Hz, 0 while unvoiced, and its confidence (0-1).
    pub pitch_hz: f32,
    pub pitch_confidence: f32,
    /// Autotune, as fractional MIDI notes (0 while off or unvoiced): the
    /// note sung, the target and the corrected note (before
    /// transposition); and the pitch classes in use (bit 0 = C).
    pub tune_sung: f32,
    pub tune_target: f32,
    pub tune_out: f32,
    pub tune_mask: u16,
    /// A frozen grain is sounding (Creative mode).
    pub frozen: bool,
    /// Notes the harmony voices sing (Synth mode), bit n = note n.
    pub harmony_notes: u128,
}

/// Seconds of pitch history in the graph.
pub const HISTORY_S: f64 = 4.0;
const HISTORY_LEN: usize = 1024;

/// One frame of the pitch graph: time, then notes (0 = none).
#[derive(Debug, Clone, Copy, Default)]
struct Sample {
    t: f64,
    sung: f32,
    target: f32,
    out: f32,
}

/// Editor state kept between frames.
#[derive(Debug)]
pub struct UiState {
    /// Knob being dragged and its normalized value at press.
    drag: Option<(ParamId, f64)>,
    /// Pitch graph: ring of samples, next write, and the note the view
    /// is centred on.
    history: Vec<Sample>,
    head: usize,
    centre: f32,
    /// Scale list open.
    scale_menu: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            drag: None,
            history: vec![Sample::default(); HISTORY_LEN],
            head: 0,
            centre: 60.0,
            scale_menu: false,
        }
    }
}

impl UiState {
    /// Close an edit left open (window closed mid-drag).
    pub fn end_open_edits(&mut self, params: &dyn ParamAccess) {
        if let Some((id, _)) = self.drag.take() {
            params.end_edit(id);
        }
    }
}

#[must_use]
pub fn mode_accent(mode: VoiceMode) -> Color32 {
    match mode {
        VoiceMode::Natural => color::NATURAL,
        VoiceMode::Tune => color::TUNE,
        VoiceMode::Creative => color::CREATIVE,
        VoiceMode::Synth => color::SYNTH,
    }
}

fn mode_blurb(mode: VoiceMode) -> &'static str {
    match mode {
        VoiceMode::Natural => {
            "Unheard correction: vibrato and scoops kept, short notes left alone, formants preserved."
        }
        VoiceMode::Tune => {
            "The audible effect, from pop tune to robot: fast retune, set note transitions."
        }
        VoiceMode::Creative => {
            "Pitch and formants apart: monster, chipmunk, freeze, glitch, octaves."
        }
        VoiceMode::Synth => "Harmonies played over MIDI, vocoder, talkbox and synthetic voices.",
    }
}

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

#[must_use]
pub fn note_name(note: u8) -> String {
    format!(
        "{}{}",
        NOTE_NAMES[usize::from(note % 12)],
        i32::from(note / 12) - 1
    )
}

/// Draw the whole editor into `ui`'s full rect.
pub fn draw(ui: &mut egui::Ui, params: &dyn ParamAccess, view: &View, st: &mut UiState) {
    let full = ui.max_rect();
    let p = ui.painter().clone();
    p.rect_filled(full, 0.0, color::BG);
    let mode = VoiceMode::from_index(params.get_plain(ParamId::Mode));
    let accent = mode_accent(mode);
    let pad = space::XL;
    let inner = full.shrink(pad);

    // Header: wordmark, bypass, latency.
    let head_y = inner.top() + 18.0;
    spaced(
        &p,
        pos2(inner.left(), head_y),
        "QUACKAPELLA",
        size::TITLE + 4.0,
        4.0,
        color::TEXT,
    );
    p.line_segment(
        [
            pos2(inner.left(), head_y + 20.0),
            pos2(inner.left() + 36.0, head_y + 20.0),
        ],
        egui::Stroke::new(2.0_f32, color::QUACK),
    );
    let byp = Rect::from_min_size(pos2(inner.right() - 96.0, head_y - 15.0), vec2(96.0, 30.0));
    controls::toggle(ui, params, ParamId::Bypass, byp, "Bypass", color::QUACK);
    #[allow(clippy::cast_possible_truncation)]
    let lat_ms = if view.sample_rate > 0.0 {
        (f64::from(view.latency_samples) * 1000.0 / view.sample_rate) as f32
    } else {
        0.0
    };
    value(
        &p,
        pos2(byp.left() - space::L, head_y),
        Align2::RIGHT_CENTER,
        &format!("{lat_ms:.1} ms"),
        size::VALUE,
        color::TEXT_DIM,
    );
    let lat_caption = text(
        &p,
        pos2(byp.left() - space::L - 70.0, head_y),
        Align2::RIGHT_CENTER,
        "Latency",
        size::CAPTION,
        color::TEXT_FAINT,
    );
    // Voice range: the lowest note sets the latency.
    let range = VoiceRange::from_index(params.get_plain(ParamId::Voice));
    let voice_r = Rect::from_min_size(
        pos2(lat_caption.left() - space::XL - 150.0, head_y - 15.0),
        vec2(150.0, 30.0),
    );
    controls::stepper(
        ui,
        params,
        ParamId::Voice,
        voice_r,
        range.name(),
        color::QUACK,
    );
    let voice_caption = text(
        &p,
        pos2(voice_r.left() - space::S, head_y),
        Align2::RIGHT_CENTER,
        "Voice",
        size::CAPTION,
        color::TEXT_FAINT,
    );
    // Shorter grain attacks for low voices: less latency.
    let low_r = Rect::from_min_size(
        pos2(voice_caption.left() - space::XL - 120.0, head_y - 15.0),
        vec2(120.0, 30.0),
    );
    controls::toggle(
        ui,
        params,
        ParamId::LowLatency,
        low_r,
        "Low Latency",
        color::QUACK,
    );

    // Mode panel.
    let mode_r = Rect::from_min_size(
        pos2(inner.left(), inner.top() + 48.0),
        vec2(inner.width(), 82.0),
    );
    panel(&p, mode_r);
    text(
        &p,
        mode_r.left_top() + vec2(space::L, 22.0),
        Align2::LEFT_CENTER,
        "Mode",
        size::LABEL,
        color::TEXT_DIM,
    );
    let seg = Rect::from_min_size(
        mode_r.left_top() + vec2(space::L + 64.0, 7.0),
        vec2(mode_r.width() * 0.55, 30.0),
    );
    let names: Vec<&str> = VoiceMode::ALL.iter().map(|m| m.name()).collect();
    let accents: Vec<Color32> = VoiceMode::ALL.iter().map(|m| mode_accent(*m)).collect();
    controls::segmented(ui, params, ParamId::Mode, seg, &names, &accents);
    text(
        &p,
        pos2(seg.left(), mode_r.bottom() - 22.0),
        Align2::LEFT_CENTER,
        mode_blurb(mode),
        size::BODY,
        color::TEXT,
    );
    let formants = if mode.preserves_formants() {
        "Formants preserved"
    } else {
        "Formants free"
    };
    text(
        &p,
        pos2(mode_r.right() - space::L, mode_r.top() + 22.0),
        Align2::RIGHT_CENTER,
        formants,
        size::CAPTION,
        accent,
    );

    // Tune panel and pitch graph.
    let mid_top = mode_r.bottom() + space::M;
    let mid_h = 318.0;
    let tune_r = Rect::from_min_size(pos2(inner.left(), mid_top), vec2(452.0, mid_h));
    tune_panel(ui, params, st, view, tune_r, accent, mode);
    let graph_r = Rect::from_min_max(
        pos2(tune_r.right() + space::M, mid_top),
        pos2(inner.right(), mid_top + mid_h),
    );
    let now = ui.input(|i| i.time);
    if now > 0.0 {
        st.record(view, now);
    }
    if mode == VoiceMode::Creative {
        creative_panel(ui, params, st, view, graph_r, accent);
    } else if mode == VoiceMode::Synth {
        harmony_panel(ui, params, st, view, graph_r, accent);
    } else {
        pitch_graph(&p, graph_r, view, st, accent);
    }

    // Bottom row: voice, level, MIDI.
    let row_top = tune_r.bottom() + space::M;
    let foot_h = 26.0;
    let row_h = inner.bottom() - foot_h - row_top;
    let voice_r = Rect::from_min_size(pos2(inner.left(), row_top), vec2(452.0, row_h));
    voice_panel(ui, params, st, voice_r, accent);
    let level_r = Rect::from_min_size(
        pos2(voice_r.right() + space::M, row_top),
        vec2(250.0, row_h),
    );
    panel(&p, level_r);
    text(
        &p,
        level_r.left_top() + vec2(space::L, 20.0),
        Align2::LEFT_CENTER,
        "Level",
        size::LABEL,
        color::TEXT_DIM,
    );
    let db = |v: f64| format!("{v:+.1}");
    let pct = |v: f64| format!("{v:.0}");
    let ky = level_r.center().y + 6.0;
    let kw = level_r.width() / 3.0;
    let knobs: [(ParamId, &dyn Fn(f64) -> String); 3] = [
        (ParamId::InputGain, &db),
        (ParamId::Mix, &pct),
        (ParamId::OutputGain, &db),
    ];
    for (i, (id, fmt)) in knobs.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = level_r.left() + kw * (i as f32 + 0.5);
        controls::knob_sized(ui, params, st, id, pos2(cx, ky), 70.0, accent, fmt);
    }

    // MIDI panel.
    let midi_r = Rect::from_min_max(
        pos2(level_r.right() + space::M, row_top),
        pos2(inner.right(), level_r.bottom()),
    );
    panel(&p, midi_r);
    text(
        &p,
        midi_r.left_top() + vec2(space::L, 20.0),
        Align2::LEFT_CENTER,
        "MIDI In",
        size::LABEL,
        color::TEXT_DIM,
    );
    let latest = (0..128u8)
        .rev()
        .find(|n| view.held_notes & (1u128 << n) != 0);
    let count = view.held_notes.count_ones();
    let status = match latest {
        None => "No note".to_owned(),
        Some(n) if count == 1 => note_name(n),
        Some(n) => format!("{} (+{})", note_name(n), count - 1),
    };
    value(
        &p,
        midi_r.right_top() + vec2(-space::L, 20.0),
        Align2::RIGHT_CENTER,
        &status,
        size::VALUE,
        if latest.is_some() {
            accent
        } else {
            color::TEXT_FAINT
        },
    );
    let kb = Rect::from_min_max(
        pos2(midi_r.left() + space::L, midi_r.top() + 40.0),
        pos2(midi_r.right() - space::L, midi_r.bottom() - space::L),
    );
    keyboard(&p, kb, view.held_notes, accent);

    // Footer: the DuckDSP mark.
    let mark_h = 14.0;
    let mark = Rect::from_min_size(
        pos2(inner.left(), inner.bottom() - mark_h),
        vec2(brand::Brand::DuckDsp.width_for(mark_h), mark_h),
    );
    brand::paint(ui, brand::Brand::DuckDsp, mark, color::TEXT_FAINT);
    text(
        &p,
        pos2(inner.right(), mark.center().y),
        Align2::RIGHT_CENTER,
        concat!("v", env!("CARGO_PKG_VERSION")),
        size::CAPTION,
        color::TEXT_FAINT,
    );

    // Safety net: a drag cut short (window lost focus) ends its edit.
    if st.drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
        st.end_open_edits(params);
    }
}

/// Pitch, fine and formant knobs, with what the setting does.
fn voice_panel(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    r: Rect,
    accent: Color32,
) {
    let p = ui.painter();
    panel(p, r);
    text(
        p,
        r.left_top() + vec2(space::L, 20.0),
        Align2::LEFT_CENTER,
        "Voice",
        size::LABEL,
        color::TEXT_DIM,
    );
    let shift = params.get_plain(ParamId::Pitch) + params.get_plain(ParamId::Fine) / 100.0;
    let formant = params.get_plain(ParamId::Formant);
    let (line1, line2) = voice_summary(shift, formant);
    value(
        p,
        r.right_top() + vec2(-space::L, 20.0),
        Align2::RIGHT_CENTER,
        &line1,
        size::VALUE,
        if shift.abs() < 0.005 {
            color::TEXT_DIM
        } else {
            accent
        },
    );
    text(
        p,
        pos2(r.center().x, r.bottom() - 13.0),
        Align2::CENTER_CENTER,
        &line2,
        size::CAPTION,
        color::TEXT_FAINT,
    );
    let st_fmt = |v: f64| format!("{v:+.1}");
    let ct_fmt = |v: f64| format!("{v:+.0}");
    let kw = r.width() / 3.0;
    let ky = r.center().y - 4.0;
    let knobs: [(ParamId, &dyn Fn(f64) -> String); 3] = [
        (ParamId::Pitch, &st_fmt),
        (ParamId::Fine, &ct_fmt),
        (ParamId::Formant, &st_fmt),
    ];
    for (i, (id, fmt)) in knobs.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = r.left() + kw * (i as f32 + 0.5);
        controls::knob_sized(ui, params, st, id, pos2(cx, ky), 70.0, accent, fmt);
    }
}

/// The Creative mode's effects: layers, formant follow, robot, freeze,
/// glitch. Takes the pitch graph's place in that mode.
fn creative_panel(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    view: &View,
    r: Rect,
    accent: Color32,
) {
    let p = ui.painter();
    panel(p, r);
    text(
        p,
        r.left_top() + vec2(space::L, 22.0),
        Align2::LEFT_CENTER,
        "Manipulate",
        size::LABEL,
        color::TEXT_DIM,
    );
    // Freeze: the big switch, with its state.
    let fz = Rect::from_min_size(
        pos2(r.right() - space::L - 120.0, r.top() + 8.0),
        vec2(120.0, 28.0),
    );
    controls::toggle(ui, params, ParamId::Freeze, fz, "Freeze", accent);
    let asked = params.get_plain(ParamId::Freeze) >= 0.5;
    let state = match (asked, view.frozen) {
        (_, true) if view.held_notes != 0 => "Frozen · played by MIDI",
        (_, true) => "Frozen",
        (true, false) => "Waiting for a sung note",
        (false, false) => "Holds the last sung grain",
    };
    text(
        p,
        pos2(fz.left() - space::M, fz.center().y),
        Align2::RIGHT_CENTER,
        state,
        size::CAPTION,
        if view.frozen {
            accent
        } else {
            color::TEXT_FAINT
        },
    );

    let pct = |v: f64| format!("{v:.0}");
    let off = |v: f64| {
        if v < 0.5 {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let knob_d = 64.0;
    let row1 = r.top() + 100.0;
    let kw = (r.width() - 2.0 * space::L) / 5.0;
    let knobs: [(ParamId, &dyn Fn(f64) -> String); 5] = [
        (ParamId::Sub, &off),
        (ParamId::OctaveDown, &off),
        (ParamId::OctaveUp, &off),
        (ParamId::FormantFollow, &pct),
        (ParamId::Robot, &off),
    ];
    for (i, (id, fmt)) in knobs.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = r.left() + space::L + kw * (i as f32 + 0.5);
        controls::knob_sized(ui, params, st, id, pos2(cx, row1), knob_d, accent, fmt);
    }
    // Group captions over the knobs.
    let cap_y = row1 - knob_d * 0.5 - 14.0;
    text(
        p,
        pos2(r.left() + space::L + kw * 1.5, cap_y),
        Align2::CENTER_CENTER,
        "Layers",
        size::CAPTION - 1.0,
        color::TEXT_FAINT,
    );
    let follow = params.get_plain(ParamId::FormantFollow);
    let shift = params.get_plain(ParamId::Pitch);
    let character = if follow < 1.0 || shift.abs() < 0.5 {
        "Formants in place"
    } else if shift > 0.0 {
        "Chipmunk"
    } else {
        "Monster"
    };
    text(
        p,
        pos2(r.left() + space::L + kw * 3.5, cap_y),
        Align2::CENTER_CENTER,
        character,
        size::CAPTION - 1.0,
        if character == "Formants in place" {
            color::TEXT_FAINT
        } else {
            accent
        },
    );
    text(
        p,
        pos2(r.left() + space::L + kw * 4.5, cap_y),
        Align2::CENTER_CENTER,
        if params.get_plain(ParamId::Robot) >= 0.5 {
            "One note per phrase"
        } else {
            "Monotone"
        },
        size::CAPTION - 1.0,
        color::TEXT_FAINT,
    );

    // Glitch: density, effect, slice length, hold.
    let row2 = r.bottom() - 66.0;
    let gx = r.left() + space::L + kw * 0.5;
    controls::knob_sized(
        ui,
        params,
        st,
        ParamId::Glitch,
        pos2(gx, row2),
        knob_d,
        accent,
        &off,
    );
    let left = gx + kw * 0.5 + space::S;
    let width = r.right() - space::L - left;
    let modes: Vec<&str> = GlitchMode::ALL.iter().map(|g| g.name()).collect();
    let mode_r = Rect::from_min_size(pos2(left, row2 - 50.0), vec2(width, 28.0));
    controls::segmented(ui, params, ParamId::GlitchMode, mode_r, &modes, &[accent]);
    let rates: Vec<&str> = GlitchRate::ALL.iter().map(|g| g.name()).collect();
    let hold_w = 84.0;
    let rate_r = Rect::from_min_size(
        pos2(left, mode_r.bottom() + space::S),
        vec2(width - hold_w - space::S, 28.0),
    );
    controls::segmented(ui, params, ParamId::GlitchRate, rate_r, &rates, &[accent]);
    let hold_r = Rect::from_min_size(
        pos2(rate_r.right() + space::S, rate_r.top()),
        vec2(hold_w, 28.0),
    );
    controls::toggle(ui, params, ParamId::GlitchHold, hold_r, "Hold", accent);
    // The bar's steps: the glitched ones lit, the same every bar.
    let rate = GlitchRate::from_index(params.get_plain(ParamId::GlitchRate));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (4.0 / rate.beats()).round() as u32;
    let amount = params.get_plain(ParamId::Glitch) / 100.0;
    let hold = params.get_plain(ParamId::GlitchHold) >= 0.5;
    let strip = Rect::from_min_size(
        pos2(left, rate_r.bottom() + space::S + 2.0),
        vec2(width, 8.0),
    );
    #[allow(clippy::cast_precision_loss)]
    let cell = strip.width() / steps as f32;
    for i in 0..steps {
        #[allow(clippy::cast_precision_loss)]
        let c = Rect::from_min_size(
            pos2(strip.left() + cell * i as f32 + 1.0, strip.top()),
            vec2(cell - 2.0, strip.height()),
        );
        let on = hold || glitch_step(i, steps, amount);
        let fill = if on {
            accent
        } else if i % (steps / 4).max(1) == 0 {
            color::TEXT_FAINT.gamma_multiply(0.5)
        } else {
            color::TEXT_FAINT.gamma_multiply(0.25)
        };
        p.rect_filled(c, 2.0, fill);
    }
    text(
        p,
        pos2(left + 2.0, strip.bottom() + 14.0),
        Align2::LEFT_CENTER,
        "Same steps every bar  ·  Hold: every step  ·  MIDI plays the freeze",
        size::CAPTION - 1.0,
        color::TEXT_FAINT,
    );
}

/// The Synth mode's harmony voices: source, intervals, levels and what
/// makes them sound like several singers. Takes the pitch graph's place
/// in that mode.
fn harmony_panel(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    view: &View,
    r: Rect,
    accent: Color32,
) {
    let p = ui.painter();
    panel(p, r);
    text(
        p,
        r.left_top() + vec2(space::L, 22.0),
        Align2::LEFT_CENTER,
        "Harmony",
        size::LABEL,
        color::TEXT_DIM,
    );
    let mode = HarmonyMode::from_index(params.get_plain(ParamId::Harmony));
    let seg = Rect::from_min_size(
        pos2(r.right() - space::L - 280.0, r.top() + 8.0),
        vec2(280.0, 28.0),
    );
    let names: Vec<&str> = HarmonyMode::ALL.iter().map(|m| m.name()).collect();
    controls::segmented(ui, params, ParamId::Harmony, seg, &names, &[accent]);
    // The notes the voices sing now.
    let sounding: Vec<String> = (0..128u8)
        .filter(|n| view.harmony_notes & (1u128 << n) != 0)
        .map(note_name)
        .collect();
    let status = if sounding.is_empty() {
        "-".to_owned()
    } else {
        sounding.join(" ")
    };
    value(
        p,
        pos2(r.left() + space::L + 74.0, r.top() + 22.0),
        Align2::LEFT_CENTER,
        &status,
        size::VALUE,
        if sounding.is_empty() {
            color::TEXT_FAINT
        } else {
            accent
        },
    );

    let knob_d = 58.0;
    let kw = (r.width() - 2.0 * space::L) / 6.0;
    let row1 = r.top() + 104.0;
    let cap_y = row1 - knob_d * 0.5 - 16.0;
    let intervals_on = matches!(mode, HarmonyMode::Scale | HarmonyMode::Chord);
    let key = note_name(params.get_plain(ParamId::Key).round().clamp(0.0, 11.0) as u8 + 60);
    let key = key.trim_end_matches(char::is_numeric).to_owned();
    let scale = Scale::from_index(params.get_plain(ParamId::Scale));
    let scale = if scale == Scale::Chromatic {
        Scale::Major
    } else {
        scale
    };
    let chord: Vec<String> = (0..12u8)
        .filter(|n| held_class(view.held_notes, *n))
        .map(|n| {
            note_name(n + 60)
                .trim_end_matches(char::is_numeric)
                .to_owned()
        })
        .collect();
    let caption = match mode {
        HarmonyMode::Off => "Harmonies off".to_owned(),
        HarmonyMode::Midi => "Every held MIDI note is a voice".to_owned(),
        HarmonyMode::Scale => format!("Steps of {key} {} from the lead", scale.name()),
        HarmonyMode::Chord if chord.is_empty() => {
            format!("Hold a chord · until then, {key} {}", scale.name())
        }
        HarmonyMode::Chord => format!("Nearest notes of {}", chord.join(" ")),
    };
    text(
        p,
        pos2(r.left() + space::L + kw * 2.0, cap_y),
        Align2::CENTER_CENTER,
        &caption,
        size::CAPTION - 1.0,
        if mode == HarmonyMode::Off {
            color::TEXT_FAINT
        } else {
            accent
        },
    );
    let interval = |v: f64| interval_short(interval_steps(v));
    let dim = accent.gamma_multiply(0.35);
    for (i, id) in [
        ParamId::Harmony1,
        ParamId::Harmony2,
        ParamId::Harmony3,
        ParamId::Harmony4,
    ]
    .into_iter()
    .enumerate()
    {
        #[allow(clippy::cast_precision_loss)]
        let cx = r.left() + space::L + kw * (i as f32 + 0.5);
        let label = format!("Voice {}", i + 1);
        controls::knob_named(
            ui,
            params,
            st,
            id,
            pos2(cx, row1),
            knob_d,
            id.spec().kind.min(),
            if intervals_on { accent } else { dim },
            &interval,
            &label,
        );
    }
    let pct = |v: f64| format!("{v:.0}");
    let off = |v: f64| {
        if v < 0.5 {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let levels: [(ParamId, &str); 2] = [(ParamId::Lead, "Lead"), (ParamId::HarmonyLevel, "Voices")];
    for (i, (id, label)) in levels.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = r.left() + space::L + kw * (i as f32 + 4.5);
        controls::knob_named(
            ui,
            params,
            st,
            id,
            pos2(cx, row1),
            knob_d,
            0.0,
            accent,
            &off,
            label,
        );
    }
    text(
        p,
        pos2(r.left() + space::L + kw * 5.0, cap_y),
        Align2::CENTER_CENTER,
        "Levels",
        size::CAPTION - 1.0,
        color::TEXT_FAINT,
    );

    // Close voicing and latch, between the rows (MIDI notes).
    let tog_y = row1 + knob_d * 0.5 + 34.0;
    let close_r = Rect::from_min_size(pos2(r.left() + space::L + 8.0, tog_y), vec2(132.0, 26.0));
    controls::toggle(
        ui,
        params,
        ParamId::CloseVoicing,
        close_r,
        "Close Voicing",
        accent,
    );
    let latch_r = Rect::from_min_size(pos2(close_r.right() + space::S, tog_y), vec2(84.0, 26.0));
    controls::toggle(ui, params, ParamId::Latch, latch_r, "Latch", accent);
    text(
        p,
        pos2(latch_r.right() + space::M, latch_r.center().y),
        Align2::LEFT_CENTER,
        match mode {
            HarmonyMode::Midi => "The sustain pedal holds notes too",
            HarmonyMode::Chord => "Latch or pedal hold the chord",
            _ => "For the MIDI and Chord modes",
        },
        size::CAPTION - 1.0,
        color::TEXT_FAINT,
    );

    // Second row: what makes several singers out of one, and the doubler.
    let row2 = r.bottom() - 60.0;
    let ms = |v: f64| {
        if v < 0.5 {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    type Knob<'a> = (ParamId, &'a str, &'a dyn Fn(f64) -> String);
    let knobs: [Knob; 6] = [
        (ParamId::Spread, "Spread", &pct),
        (ParamId::HarmonyHumanize, "Humanize", &off),
        (ParamId::Character, "Character", &off),
        (ParamId::Expression, "Expression", &pct),
        (ParamId::Glide, "Glide", &ms),
        (ParamId::Double, "Double", &off),
    ];
    for (i, (id, label, fmt)) in knobs.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = r.left() + space::L + kw * (i as f32 + 0.5);
        controls::knob_named(
            ui,
            params,
            st,
            id,
            pos2(cx, row2),
            knob_d,
            0.0,
            accent,
            fmt,
            label,
        );
    }
}

fn held_class(held: u128, class: u8) -> bool {
    (0..128u8).any(|n| n % 12 == class && held & (1u128 << n) != 0)
}

/// Key, scale, target source, note switches and the retune knobs.
fn tune_panel(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    view: &View,
    r: Rect,
    accent: Color32,
    mode: VoiceMode,
) {
    let p = ui.painter();
    panel(p, r);
    text(
        p,
        r.left_top() + vec2(space::L, 22.0),
        Align2::LEFT_CENTER,
        "Tune",
        size::LABEL,
        color::TEXT_DIM,
    );
    let sources: Vec<&str> = Source::ALL.iter().map(|s| s.name()).collect();
    let src_r = Rect::from_min_size(
        pos2(r.right() - space::L - 260.0, r.top() + 8.0),
        vec2(260.0, 28.0),
    );
    controls::segmented(ui, params, ParamId::Source, src_r, &sources, &[accent]);
    let source = Source::from_index(params.get_plain(ParamId::Source));
    let gate_on = params.get_plain(ParamId::MidiGate) >= 0.5;
    let gate_r = Rect::from_min_size(
        pos2(src_r.left() - space::S - 96.0, src_r.top()),
        vec2(96.0, 28.0),
    );
    controls::toggle(ui, params, ParamId::MidiGate, gate_r, "MIDI Gate", accent);
    if source == Source::Scale {
        // The gate only acts on the MIDI sources.
        p.rect_filled(gate_r, 5.0, color::PANEL.gamma_multiply(0.6));
    }

    // Key and scale.
    let row = r.top() + 48.0;
    let key_r = Rect::from_min_size(
        pos2(r.left() + space::L, row),
        vec2(r.width() - 2.0 * space::L, 26.0),
    );
    controls::segmented(ui, params, ParamId::Key, key_r, &NOTE_NAMES, &[accent]);
    let scale = Scale::from_index(params.get_plain(ParamId::Scale));
    let scale_r = Rect::from_min_size(
        pos2(key_r.left(), key_r.bottom() + space::S),
        vec2(190.0, 30.0),
    );
    if controls::stepper(ui, params, ParamId::Scale, scale_r, scale.name(), accent) {
        st.scale_menu = !st.scale_menu;
    }

    // Note switches: the scale in its key, a click takes a note out.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let key = params.get_plain(ParamId::Key).round().clamp(0.0, 11.0) as u8;
    let notes_r = Rect::from_min_max(
        pos2(scale_r.right() + space::M, scale_r.top()),
        pos2(key_r.right(), scale_r.top() + 92.0),
    );
    controls::note_switches(
        ui,
        params,
        &NOTE_PARAMS,
        notes_r,
        scale_mask(key, scale),
        view.tune_mask,
        accent,
    );
    // Reset the note switches.
    let scale_notes = scale_mask(key, scale);
    let edited =
        (0..12).any(|pc| scale_notes & (1 << pc) != 0 && params.get_plain(NOTE_PARAMS[pc]) < 0.5);
    let reset_r = Rect::from_min_size(
        pos2(scale_r.left(), scale_r.bottom() + space::S),
        vec2(scale_r.width(), 26.0),
    );
    let resp = ui.interact(
        reset_r,
        ui.id().with("quack-reset-notes"),
        egui::Sense::click(),
    );
    if resp.clicked() && edited {
        for id in NOTE_PARAMS {
            if params.get_plain(id) < 0.5 {
                set_once(params, id, 1.0);
            }
        }
    }
    paint::pill(
        p,
        reset_r,
        "Reset notes",
        false,
        resp.hovered() && edited,
        accent,
    );
    if !edited {
        p.rect_filled(reset_r, 5.0, color::PANEL.gamma_multiply(0.6));
    }

    // In the MIDI sources the scale steps back: it only fills in while
    // no note is held, or not at all behind the gate.
    let (hint, hint2) = match (source, gate_on) {
        (Source::Scale, _) => ("Notes from the scale, minus the ones switched off", ""),
        (Source::Midi, false) => ("Held notes, any octave", "  ·  scale while none is held"),
        (Source::MidiExact, false) => ("Sings the held note", "  ·  scale while none is held"),
        (Source::Midi, true) => ("Held notes, any octave", "  ·  silent while none is held"),
        (Source::MidiExact, true) => ("Sings the held note", "  ·  silent while none is held"),
    };
    if source != Source::Scale {
        let veil = Rect::from_min_max(
            pos2(key_r.left() - 2.0, key_r.top() - 2.0),
            pos2(key_r.right() + 2.0, notes_r.bottom() + 2.0),
        );
        let alpha = if gate_on { 0.75 } else { 0.55 };
        p.rect_filled(veil, 6.0, color::PANEL.gamma_multiply(alpha));
    }
    let hy = notes_r.bottom() + 16.0;
    let h1 = text(
        p,
        pos2(key_r.left() + 2.0, hy),
        Align2::LEFT_CENTER,
        hint,
        size::CAPTION,
        if source == Source::Scale {
            color::TEXT_FAINT
        } else {
            accent
        },
    );
    text(
        p,
        pos2(h1.right(), hy),
        Align2::LEFT_CENTER,
        hint2,
        size::CAPTION,
        color::TEXT_FAINT,
    );
    if st.scale_menu {
        let names: Vec<&str> = Scale::ALL.iter().map(|s| s.name()).collect();
        st.scale_menu = controls::choice_menu(ui, params, ParamId::Scale, scale_r, &names, accent);
    }

    // Knobs.
    let pct = |v: f64| format!("{v:.0}");
    let ms = |v: f64| {
        if v < 0.5 {
            "Hard".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let ky = r.bottom() - 78.0;
    let kw = r.width() / 5.0;
    let trans = |v: f64| {
        if v < 0.5 {
            "Jump".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let knobs: [(ParamId, &dyn Fn(f64) -> String); 5] = [
        (ParamId::Correction, &pct),
        (ParamId::Retune, &ms),
        (ParamId::Transition, &trans),
        (ParamId::Humanize, &pct),
        (ParamId::Vibrato, &pct),
    ];
    // Natural has floors (the knobs stop there) and no transition: its
    // notes glide at the retune speed.
    let natural = mode == VoiceMode::Natural;
    for (i, (id, fmt)) in knobs.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = r.left() + kw * (i as f32 + 0.5);
        let floor = match id {
            ParamId::Retune if natural => quack_dsp::engine::NATURAL_MIN_RETUNE_S * 1000.0,
            ParamId::Vibrato if natural => quack_dsp::autotune::NATURAL_MIN_VIBRATO * 100.0,
            _ => id.spec().kind.min(),
        };
        controls::knob_floor(ui, params, st, id, pos2(cx, ky), 70.0, floor, accent, fmt);
        if natural && id == ParamId::Transition {
            let veil = Rect::from_center_size(pos2(cx, ky + 10.0), vec2(kw, 100.0));
            ui.painter()
                .rect_filled(veil, 6.0, color::PANEL.gamma_multiply(0.7));
            text(
                ui.painter(),
                pos2(cx, ky),
                Align2::CENTER_CENTER,
                "Tune only",
                size::CAPTION - 2.0,
                color::TEXT_FAINT,
            );
        }
    }
}

impl UiState {
    /// Add a frame to the pitch graph (`draw` does it with the UI's
    /// clock; public for captures).
    pub fn record(&mut self, view: &View, t: f64) {
        let sung = if view.tune_sung > 0.0 {
            view.tune_sung
        } else if view.pitch_hz > 0.0 {
            quack_dsp::pitch::hz_to_midi(view.pitch_hz)
        } else {
            0.0
        };
        let prev = self.history[(self.head + HISTORY_LEN - 1) % HISTORY_LEN];
        // One sample per frame; frames faster than 4 ms are skipped.
        if t > 0.0 && t - prev.t < 0.004 {
            return;
        }
        self.history[self.head] = Sample {
            t,
            sung,
            target: view.tune_target,
            out: view.tune_out,
        };
        self.head = (self.head + 1) % HISTORY_LEN;
        let focus = if view.tune_target > 0.0 {
            view.tune_target
        } else {
            sung
        };
        if focus > 0.0 {
            self.centre += 0.08 * (focus - self.centre);
        }
    }

    fn samples(&self) -> impl Iterator<Item = Sample> + '_ {
        (0..HISTORY_LEN).map(move |i| self.history[(self.head + i) % HISTORY_LEN])
    }
}

/// Scrolling pitch: lanes for the notes in use, the sung pitch (thin),
/// the target notes (bars) and the corrected pitch (accent), with the
/// current reading on top.
fn pitch_graph(p: &egui::Painter, r: Rect, view: &View, st: &UiState, accent: Color32) {
    const SPAN: f32 = 15.0;
    panel(p, r);
    // Reading.
    let ty = r.top() + 22.0;
    text(
        p,
        pos2(r.left() + space::L, ty),
        Align2::LEFT_CENTER,
        "Pitch",
        size::LABEL,
        color::TEXT_DIM,
    );
    let reading = nearest_note(view.pitch_hz);
    let x0 = r.left() + space::L + 52.0;
    match reading {
        Some((n, c)) => {
            value(
                p,
                pos2(x0, ty),
                Align2::LEFT_CENTER,
                &note_name(n),
                size::TITLE,
                color::TEXT,
            );
            value(
                p,
                pos2(x0 + 48.0, ty),
                Align2::LEFT_CENTER,
                &format!("{c:+3.0} ct  {:.1} Hz", view.pitch_hz),
                size::VALUE,
                color::TEXT_DIM,
            );
        }
        None => {
            text(
                p,
                pos2(x0, ty),
                Align2::LEFT_CENTER,
                "Unvoiced",
                size::CAPTION,
                color::TEXT_FAINT,
            );
        }
    }
    if view.tune_target > 0.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let t = view.tune_target.round().clamp(0.0, 127.0) as u8;
        let corr = (view.tune_out - view.tune_sung) * 100.0;
        value(
            p,
            pos2(r.right() - space::L, ty),
            Align2::RIGHT_CENTER,
            &format!("{}  {corr:+.0} ct", note_name(t)),
            size::VALUE,
            accent,
        );
        text(
            p,
            pos2(r.right() - space::L - 112.0, ty),
            Align2::RIGHT_CENTER,
            "Target",
            size::CAPTION,
            color::TEXT_FAINT,
        );
    }

    let g = Rect::from_min_max(
        pos2(r.left() + space::L, r.top() + 44.0),
        pos2(r.right() - space::L, r.bottom() - space::L),
    );
    well(p, g);
    let g = g.shrink(2.0);
    let lo = st.centre - SPAN * 0.5;
    let y_of = |note: f32| g.bottom() - (note - lo) / SPAN * g.height();
    let lane_h = g.height() / SPAN;
    let clip = p.with_clip_rect(g);
    // Lanes.
    #[allow(clippy::cast_possible_truncation)]
    let first = lo.floor() as i32;
    for n in first..=first + SPAN as i32 + 1 {
        #[allow(clippy::cast_precision_loss)]
        let y = y_of(n as f32);
        let pc = n.rem_euclid(12);
        let used = view.tune_mask & (1 << pc) != 0;
        if used {
            clip.rect_filled(
                Rect::from_center_size(pos2(g.center().x, y), vec2(g.width(), lane_h - 1.0)),
                0.0,
                color::GRID,
            );
        }
        if pc == 0 {
            clip.line_segment(
                [
                    pos2(g.left(), y + lane_h * 0.5),
                    pos2(g.right(), y + lane_h * 0.5),
                ],
                egui::Stroke::new(1.0_f32, color::GRID_STRONG),
            );
        }
        if used || pc == 0 {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let name = note_name(n.clamp(0, 127) as u8);
            clip.text(
                pos2(g.left() + 5.0, y),
                Align2::LEFT_CENTER,
                name,
                theme::value_font(size::CAPTION - 3.0),
                if used {
                    color::TEXT_FAINT
                } else {
                    color::GRID_STRONG
                },
            );
        }
    }
    // Time axis: newest at the right edge.
    let now = st.samples().map(|s| s.t).fold(0.0f64, f64::max);
    #[allow(clippy::cast_possible_truncation)]
    let x_of = |t: f64| g.right() - ((now - t) / HISTORY_S) as f32 * g.width();
    let pts: Vec<Sample> = st
        .samples()
        .filter(|s| s.t > 0.0 && now - s.t <= HISTORY_S)
        .collect();
    // Target bars, one per held target.
    let mut i = 0;
    while i < pts.len() {
        let n = pts[i].target;
        let mut j = i;
        while j + 1 < pts.len() && (pts[j + 1].target - n).abs() < 0.01 {
            j += 1;
        }
        if n > 0.0 && j > i {
            let bar = Rect::from_min_max(
                pos2(x_of(pts[i].t), y_of(n) - lane_h * 0.32),
                pos2(x_of(pts[j].t), y_of(n) + lane_h * 0.32),
            );
            clip.rect_filled(bar, 3.0, accent.gamma_multiply(0.22));
        }
        i = j + 1;
    }
    let line = |sel: fn(&Sample) -> f32, width: f32, c: Color32| {
        let mut run: Vec<egui::Pos2> = Vec::new();
        let flush = |run: &mut Vec<egui::Pos2>| {
            if run.len() > 1 {
                clip.add(egui::Shape::line(
                    std::mem::take(run),
                    egui::Stroke::new(width, c),
                ));
            }
            run.clear();
        };
        let mut last: Option<f32> = None;
        for s in &pts {
            let v = sel(s);
            if v <= 0.0 || last.is_some_and(|l| (l - v).abs() > 3.0) {
                flush(&mut run);
            }
            if v > 0.0 {
                run.push(pos2(x_of(s.t), y_of(v)));
            }
            last = (v > 0.0).then_some(v);
        }
        flush(&mut run);
    };
    line(|s| s.sung, 1.5, color::TEXT_FAINT);
    line(|s| s.out, 2.0, accent);
    // Legend.
    let ly = g.bottom() - 10.0;
    let mut x = g.right() - 8.0;
    for (label, c) in [("Corrected", accent), ("Sung", color::TEXT_FAINT)] {
        let w = text(
            p,
            pos2(x, ly),
            Align2::RIGHT_CENTER,
            label,
            size::CAPTION - 2.0,
            color::TEXT_DIM,
        );
        p.line_segment(
            [pos2(w.left() - 18.0, ly), pos2(w.left() - 5.0, ly)],
            egui::Stroke::new(2.0_f32, c),
        );
        x = w.left() - 28.0;
    }
}

/// The Voice panel's two lines: the interval, then where the formants
/// are.
#[must_use]
pub fn voice_summary(shift_st: f64, formant_st: f64) -> (String, String) {
    let line1 = if shift_st.abs() < 0.005 {
        "No transposition".to_owned()
    } else {
        let dir = if shift_st > 0.0 { "up" } else { "down" };
        format!("{:.2} semitones {dir}", shift_st.abs())
    };
    let line2 = if formant_st.abs() < 0.05 {
        "Formants in place, consonants untouched".to_owned()
    } else {
        let dir = if formant_st > 0.0 {
            "brighter"
        } else {
            "darker"
        };
        format!("Formants {formant_st:+.1} st ({dir}), pitch unchanged")
    };
    (line1, line2)
}

/// Nearest note and the offset from it in cents, for a pitch in Hz.
#[must_use]
pub fn nearest_note(hz: f32) -> Option<(u8, f32)> {
    if !(hz.is_finite() && hz > 0.0) {
        return None;
    }
    let m = quack_dsp::pitch::hz_to_midi(hz);
    let n = m.round();
    if !(0.0..=127.0).contains(&n) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((n as u8, (m - n) * 100.0))
}

/// Three octaves from C3, held notes lit (any octave folds in).
fn keyboard(p: &egui::Painter, r: Rect, held: u128, accent: Color32) {
    const LOW: u8 = 48;
    const OCTAVES: u8 = 3;
    well(p, r);
    let r = r.shrink(3.0);
    let whites: Vec<u8> = (LOW..LOW + 12 * OCTAVES)
        .filter(|n| !is_black(*n))
        .collect();
    #[allow(clippy::cast_precision_loss)]
    let ww = r.width() / whites.len() as f32;
    let lit = |n: u8| {
        // Notes outside the drawn range light their pitch class.
        (0..128u8).any(|m| {
            held & (1u128 << m) != 0
                && (m == n || (m % 12 == n % 12 && !(LOW..LOW + 12 * OCTAVES).contains(&m)))
        })
    };
    for (i, n) in whites.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let k = Rect::from_min_size(
            pos2(r.left() + ww * i as f32, r.top()),
            vec2(ww - 1.0, r.height()),
        );
        let fill = if lit(*n) {
            accent
        } else {
            Color32::from_rgb(0xC9, 0xD2, 0xDE)
        };
        p.rect_filled(k, 2.0, fill);
    }
    for (i, n) in whites.iter().enumerate() {
        let b = n + 1;
        if is_black(b) && b < LOW + 12 * OCTAVES {
            #[allow(clippy::cast_precision_loss)]
            let x = r.left() + ww * (i as f32 + 1.0);
            let k = Rect::from_min_size(
                pos2(x - ww * 0.3, r.top()),
                vec2(ww * 0.6, r.height() * 0.6),
            );
            let fill = if lit(b) { accent } else { color::BG };
            p.rect_filled(k, 2.0, fill);
        }
    }
}

fn is_black(n: u8) -> bool {
    matches!(n % 12, 1 | 3 | 6 | 8 | 10)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct Fake {
        values: RefCell<quack_dsp::params::ParamValues>,
        log: RefCell<Vec<String>>,
    }

    impl ParamAccess for Fake {
        fn get_plain(&self, id: ParamId) -> f64 {
            self.values.borrow().get(id)
        }
        fn begin_edit(&self, id: ParamId) {
            self.log
                .borrow_mut()
                .push(format!("begin {}", id.spec().key));
        }
        fn set_plain(&self, id: ParamId, plain: f64) {
            self.values.borrow_mut().set(id, plain);
        }
        fn end_edit(&self, id: ParamId) {
            self.log.borrow_mut().push(format!("end {}", id.spec().key));
        }
    }

    #[test]
    fn draws_every_mode_headless() {
        let ctx = egui::Context::default();
        theme::apply(&ctx, include_bytes!("../assets/Inter-Regular.ttf"));
        let params = Fake::default();
        let mut st = UiState::default();
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                vec2(BASE_W as f32, BASE_H as f32),
            )),
            ..Default::default()
        };
        for mode in 0..4 {
            params
                .values
                .borrow_mut()
                .set(ParamId::Mode, f64::from(mode));
            let view = View {
                held_notes: (1 << 60) | (1 << 76),
                latency_samples: 591,
                sample_rate: 48_000.0,
                pitch_hz: if mode % 2 == 0 { 0.0 } else { 223.1 },
                pitch_confidence: 0.92,
                tune_sung: 57.2,
                tune_target: 57.0,
                tune_out: 57.02,
                tune_mask: 0b1010_1011_0101,
                frozen: mode == 2,
                harmony_notes: if mode == 3 { 1 << 64 } else { 0 },
            };
            for _ in 0..2 {
                let _ = ctx.run_ui(input(), |ui| draw(ui, &params, &view, &mut st));
            }
        }
        assert!(params.log.borrow().is_empty());
    }

    #[test]
    fn voice_summaries() {
        assert_eq!(voice_summary(0.0, 0.0).0, "No transposition");
        assert_eq!(voice_summary(-12.0, 0.0).0, "12.00 semitones down");
        assert!(voice_summary(7.2, 3.0).1.starts_with("Formants +3.0 st"));
    }

    #[test]
    fn note_names() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(0), "C-1");
    }

    #[test]
    fn nearest_notes() {
        let (n, c) = nearest_note(223.1).unwrap();
        assert_eq!(n, 57);
        assert!((c - 24.2).abs() < 0.5, "{c}");
        assert_eq!(nearest_note(0.0), None);
        assert_eq!(nearest_note(f32::NAN), None);
    }
}
