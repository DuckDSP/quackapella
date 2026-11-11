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
//! (transposition, fine tune, formant shift). Panels for autotune,
//! harmonies and vocoder come with their DSP steps (roadmap step 8 for
//! presets, macros and MIDI learn).

mod brand;
mod controls;
mod paint;
pub mod theme;

use egui::{Align2, Color32, Rect, pos2, vec2};
use quack_dsp::{ParamId, VoiceMode};

use paint::{panel, spaced, text, value, well};
use theme::{color, size, space};

/// Editor size in points (fixed for now; zoom comes with step 8, with
/// the AUv2 resize patch already in `vendor/`).
pub const BASE_W: u32 = 760;
pub const BASE_H: u32 = 680;

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
}

/// Editor state kept between frames.
#[derive(Debug, Default)]
pub struct UiState {
    /// Knob being dragged and its normalized value at press.
    drag: Option<(ParamId, f64)>,
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
            "Transparent correction and transposition, formants preserved, even for large intervals."
        }
        VoiceMode::Tune => {
            "Pitch correction to a scale or to MIDI notes, from gentle to hard tune."
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
    text(
        &p,
        pos2(byp.left() - space::L - 70.0, head_y),
        Align2::RIGHT_CENTER,
        "Latency",
        size::CAPTION,
        color::TEXT_FAINT,
    );

    // Mode panel.
    let mode_r = Rect::from_min_size(
        pos2(inner.left(), inner.top() + 52.0),
        vec2(inner.width(), 96.0),
    );
    panel(&p, mode_r);
    text(
        &p,
        mode_r.left_top() + vec2(space::L, 20.0),
        Align2::LEFT_CENTER,
        "Mode",
        size::LABEL,
        color::TEXT_DIM,
    );
    let seg = Rect::from_min_size(
        mode_r.left_top() + vec2(space::L + 64.0, 6.0),
        vec2(mode_r.width() - 2.0 * space::L - 64.0, 30.0),
    );
    let names: Vec<&str> = VoiceMode::ALL.iter().map(|m| m.name()).collect();
    let accents: Vec<Color32> = VoiceMode::ALL.iter().map(|m| mode_accent(*m)).collect();
    controls::segmented(ui, params, ParamId::Mode, seg, &names, &accents);
    text(
        &p,
        pos2(seg.left(), mode_r.bottom() - 30.0),
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
        pos2(seg.left(), mode_r.bottom() - 12.0),
        Align2::LEFT_CENTER,
        formants,
        size::CAPTION,
        accent,
    );

    // Pitch strip.
    let pitch_r = Rect::from_min_size(
        pos2(inner.left(), mode_r.bottom() + space::L),
        vec2(inner.width(), 64.0),
    );
    pitch_strip(&p, pitch_r, view, accent);

    // Voice panel: transposition and formants.
    let voice_r = Rect::from_min_size(
        pos2(inner.left(), pitch_r.bottom() + space::L),
        vec2(inner.width(), 148.0),
    );
    voice_panel(ui, params, st, voice_r, accent);

    // Level panel.
    let row_top = voice_r.bottom() + space::L;
    let foot_h = 28.0;
    let row_h = inner.bottom() - foot_h - row_top;
    let level_r = Rect::from_min_size(
        pos2(inner.left(), row_top),
        vec2(inner.width() * 0.52, row_h),
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
    let ky = level_r.center().y + 2.0;
    let kw = level_r.width() / 3.0;
    let knobs: [(ParamId, &dyn Fn(f64) -> String); 3] = [
        (ParamId::InputGain, &db),
        (ParamId::Mix, &pct),
        (ParamId::OutputGain, &db),
    ];
    for (i, (id, fmt)) in knobs.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = level_r.left() + kw * (i as f32 + 0.5);
        controls::knob(ui, params, st, id, pos2(cx, ky), accent, fmt);
    }

    // MIDI panel.
    let midi_r = Rect::from_min_max(
        pos2(level_r.right() + space::L, row_top),
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
        None => "No note held".to_owned(),
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
        pos2(midi_r.left() + space::L, midi_r.top() + 44.0),
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
    let st_fmt = |v: f64| format!("{v:+.1}");
    let ct_fmt = |v: f64| format!("{v:+.0}");
    // Same columns as the Level knobs below.
    let kw = r.width() * 0.52 / 3.0;
    let ky = r.center().y + 4.0;
    let knobs: [(ParamId, &dyn Fn(f64) -> String); 3] = [
        (ParamId::Pitch, &st_fmt),
        (ParamId::Fine, &ct_fmt),
        (ParamId::Formant, &st_fmt),
    ];
    for (i, (id, fmt)) in knobs.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = r.left() + kw * (i as f32 + 0.5);
        controls::knob(ui, params, st, id, pos2(cx, ky), accent, fmt);
    }

    let shift = params.get_plain(ParamId::Pitch) + params.get_plain(ParamId::Fine) / 100.0;
    let formant = params.get_plain(ParamId::Formant);
    let x = r.left() + r.width() * 0.52 + space::L;
    let (line1, line2) = voice_summary(shift, formant);
    let p = ui.painter();
    value(
        p,
        pos2(x, ky - 12.0),
        Align2::LEFT_CENTER,
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
        pos2(x, ky + 12.0),
        Align2::LEFT_CENTER,
        &line2,
        size::CAPTION,
        color::TEXT_FAINT,
    );
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

/// Detected note, cents, Hz and confidence, with a ±50 cent meter.
fn pitch_strip(p: &egui::Painter, r: Rect, view: &View, accent: Color32) {
    panel(p, r);
    let cy = r.center().y;
    text(
        p,
        pos2(r.left() + space::L, cy),
        Align2::LEFT_CENTER,
        "Pitch",
        size::LABEL,
        color::TEXT_DIM,
    );
    let reading = nearest_note(view.pitch_hz);
    let note_x = r.left() + 84.0;
    match reading {
        Some((n, c)) => {
            value(
                p,
                pos2(note_x, cy),
                Align2::LEFT_CENTER,
                &note_name(n),
                size::READOUT,
                accent,
            );
            value(
                p,
                pos2(note_x + 76.0, cy - 9.0),
                Align2::LEFT_CENTER,
                &format!("{c:+.0} ct"),
                size::VALUE,
                color::TEXT,
            );
            value(
                p,
                pos2(note_x + 76.0, cy + 10.0),
                Align2::LEFT_CENTER,
                &format!("{:.1} Hz", view.pitch_hz),
                size::VALUE,
                color::TEXT_DIM,
            );
        }
        None => {
            value(
                p,
                pos2(note_x, cy),
                Align2::LEFT_CENTER,
                "-",
                size::READOUT,
                color::TEXT_FAINT,
            );
            text(
                p,
                pos2(note_x + 76.0, cy),
                Align2::LEFT_CENTER,
                "Unvoiced",
                size::CAPTION,
                color::TEXT_FAINT,
            );
        }
    }

    // Meter.
    let m = Rect::from_min_max(
        pos2(r.left() + 280.0, cy - 10.0),
        pos2(r.right() - 150.0, cy + 10.0),
    );
    well(p, m);
    for k in -2..=2_i8 {
        let x = m.center().x + f32::from(k) * m.width() / 4.0;
        let h = if k == 0 { 7.0 } else { 4.0 };
        p.line_segment(
            [pos2(x, m.top() + 2.0), pos2(x, m.top() + 2.0 + h)],
            egui::Stroke::new(1.0_f32, color::GRID_STRONG),
        );
        p.line_segment(
            [pos2(x, m.bottom() - 2.0 - h), pos2(x, m.bottom() - 2.0)],
            egui::Stroke::new(1.0_f32, color::GRID_STRONG),
        );
    }
    if let Some((_, c)) = reading {
        let x = m.center().x + c.clamp(-50.0, 50.0) / 100.0 * (m.width() - 8.0);
        let alpha = 0.35 + 0.65 * view.pitch_confidence.clamp(0.0, 1.0);
        p.rect_filled(
            Rect::from_center_size(pos2(x, cy), vec2(4.0, m.height() - 4.0)),
            2.0,
            accent.gamma_multiply(alpha),
        );
    }

    // Confidence.
    text(
        p,
        pos2(r.right() - space::L - 44.0, cy),
        Align2::RIGHT_CENTER,
        "Confidence",
        size::CAPTION,
        color::TEXT_FAINT,
    );
    let conf = if reading.is_some() {
        format!("{:.0}%", view.pitch_confidence * 100.0)
    } else {
        "-".to_owned()
    };
    value(
        p,
        pos2(r.right() - space::L, cy),
        Align2::RIGHT_CENTER,
        &conf,
        size::VALUE,
        color::TEXT_DIM,
    );
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
                latency_samples: 480,
                sample_rate: 48_000.0,
                pitch_hz: if mode % 2 == 0 { 0.0 } else { 223.1 },
                pitch_confidence: 0.92,
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
