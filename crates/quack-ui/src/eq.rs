//! The output EQ on the FX page: the graph (spectrum of the
//! transformed sound, the EQ's curve, one handle per band) and the EQ
//! tab's knobs (the selected band, the exciter).
//!
//! The spectrum is computed here, on the UI thread, from the engine's
//! analysis tap ([`quack_dsp::tap`]): the audio thread only lends its
//! samples, so the graph costs it nothing and adds no latency.

use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use quack_dsp::ParamId;
use quack_dsp::assist::fft;
use quack_dsp::fx::{EQ_BANDS, EqCurve, EqSettings, LOW_CUT_OFF_HZ};
use quack_dsp::params::EQ_BELL_PARAMS;
use quack_dsp::tap::Taps;

use crate::controls;
use crate::paint::{panel, text, value, well};
use crate::theme::{color, size, space};
use crate::{FxTab, ParamAccess, UiState, View, set_once};

/// Samples per spectrum (43 ms at 48 kHz).
const FFT_LEN: usize = 2048;
/// Points of the drawn spectrum, evenly spaced in octaves.
const POINTS: usize = 128;
const F_MIN: f32 = 30.0;
const F_MAX: f32 = 20_000.0;
/// The curve's scale: ± this many dB over the plot's height.
const GAIN_SPAN: f32 = 15.0;
/// The spectrum's scale, bottom and top of the plot, in dBFS (after
/// the display tilt).
const FLOOR_DB: f32 = -84.0;
const TOP_DB: f32 = -6.0;
/// Display tilt, dB per octave around 1 kHz: a voice's spectrum falls
/// with frequency, and reads flat-ish with it.
const TILT_DB: f32 = 3.0;
const HANDLE_R: f32 = 10.0;

/// One band of the EQ as the editor sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Band {
    pub name: &'static str,
    pub freq: ParamId,
    pub gain: Option<ParamId>,
    pub q: Option<ParamId>,
}

pub(crate) const BANDS: [Band; EQ_BANDS] = [
    Band {
        name: "Cut",
        freq: ParamId::EqLowCut,
        gain: None,
        q: None,
    },
    Band {
        name: "Low",
        freq: ParamId::EqLowShelfFreq,
        gain: Some(ParamId::EqLowShelfGain),
        q: None,
    },
    bell(0),
    bell(1),
    bell(2),
    Band {
        name: "High",
        freq: ParamId::EqHighShelfFreq,
        gain: Some(ParamId::EqHighShelfGain),
        q: None,
    },
];

const fn bell(i: usize) -> Band {
    Band {
        name: "Bell",
        freq: EQ_BELL_PARAMS[i][0],
        gain: Some(EQ_BELL_PARAMS[i][1]),
        q: Some(EQ_BELL_PARAMS[i][2]),
    }
}

/// The EQ's settings from the parameters (as the engine reads them).
#[must_use]
pub(crate) fn settings(params: &dyn ParamAccess) -> EqSettings {
    #[allow(clippy::cast_possible_truncation)]
    let f = |id: ParamId| params.get_plain(id) as f32;
    EqSettings {
        low_cut_hz: f(ParamId::EqLowCut),
        low_shelf_hz: f(ParamId::EqLowShelfFreq),
        low_shelf_db: f(ParamId::EqLowShelfGain),
        bells: EQ_BELL_PARAMS.map(|ids| (f(ids[0]), f(ids[1]), f(ids[2]))),
        high_shelf_hz: f(ParamId::EqHighShelfFreq),
        high_shelf_db: f(ParamId::EqHighShelfGain),
    }
}

/// Frequency of spectrum point `i`.
fn point_hz(i: f32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let t = i / (POINTS - 1) as f32;
    F_MIN * (F_MAX / F_MIN).powf(t)
}

/// The spectrum drawn behind the curve, smoothed between frames.
#[derive(Debug, Clone)]
pub(crate) struct Spectrum {
    re: Vec<f32>,
    im: Vec<f32>,
    window: Vec<f32>,
    /// Level of each point, dBFS (a full-scale sine reads 0).
    db: [f32; POINTS],
    /// Samples the tap had written at the last frame.
    seen: usize,
}

impl Default for Spectrum {
    fn default() -> Self {
        #[allow(clippy::cast_precision_loss)]
        let window = (0..FFT_LEN)
            .map(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_LEN - 1) as f32).cos()
            })
            .collect();
        Self {
            re: vec![0.0; FFT_LEN],
            im: vec![0.0; FFT_LEN],
            window,
            db: [-200.0; POINTS],
            seen: 0,
        }
    }
}

impl Spectrum {
    /// One frame: the last samples of `taps.layers`, analysed and
    /// folded into the drawn levels (fast up, slow down).
    fn update(&mut self, taps: &Taps, sample_rate: f32) {
        let written = taps.layers.written();
        let running = written != self.seen;
        self.seen = written;
        if !running {
            // The engine is stopped: the picture falls away.
            for d in &mut self.db {
                *d += (-200.0 - *d) * 0.12;
            }
            return;
        }
        taps.layers.read(&mut self.re);
        for ((r, i), w) in self.re.iter_mut().zip(&mut self.im).zip(&self.window) {
            *r *= w;
            *i = 0.0;
        }
        fft(&mut self.re, &mut self.im);
        #[allow(clippy::cast_precision_loss)]
        let n = FFT_LEN as f32;
        // A full-scale sine reads 0 dB (the Hann window halves it).
        let norm = 20.0 * (4.0 / n).log10();
        let power = |k: usize| self.re[k].powi(2) + self.im[k].powi(2);
        let top = FFT_LEN / 2 - 1;
        for (i, slot) in self.db.iter_mut().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let i = i as f32;
            let (lo, hi) = (
                point_hz(i - 0.5) * n / sample_rate,
                point_hz(i + 0.5) * n / sample_rate,
            );
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let p = if hi - lo < 1.0 {
                // Under one bin wide: between the two nearest bins.
                let c = (0.5 * (lo + hi)).clamp(0.0, top as f32);
                let k = (c.floor() as usize).min(top - 1);
                #[allow(clippy::cast_precision_loss)]
                let t = c - k as f32;
                power(k) * (1.0 - t) + power(k + 1) * t
            } else {
                let (a, b) = (
                    (lo.ceil() as usize).min(top),
                    (hi.floor() as usize).min(top),
                );
                (a..=b.max(a)).map(power).fold(0.0, f32::max)
            };
            let hz = point_hz(i);
            let db = 10.0 * p.max(1e-20).log10() + norm + TILT_DB * (hz / 1000.0).log2();
            if *slot < -150.0 {
                // First sound: no rise from nothing.
                *slot = db;
            } else {
                let c = if db > *slot { 0.5 } else { 0.12 };
                *slot += (db - *slot) * c;
            }
        }
    }
}

/// What the EQ page keeps between frames.
#[derive(Debug, Clone, Default)]
pub(crate) struct EqUi {
    /// Band shown on the EQ tab's knobs.
    pub band: usize,
    /// Band whose handle is being dragged.
    drag: Option<usize>,
    spectrum: Spectrum,
    pub taps: Option<Taps>,
}

impl EqUi {
    /// A handle is being dragged.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

/// Close a drag cut short (window lost focus, plugin closed).
pub(crate) fn end_drag(params: &dyn ParamAccess, st: &mut UiState) {
    if let Some(i) = st.eq.drag.take() {
        let b = BANDS[i];
        params.end_edit(b.freq);
        if let Some(g) = b.gain {
            params.end_edit(g);
        }
    }
}

fn hz_text(v: f64) -> String {
    if v >= 10_000.0 {
        format!("{:.0}k", v / 1000.0)
    } else if v >= 1000.0 {
        format!("{:.1}k", v / 1000.0)
    } else {
        format!("{v:.0}")
    }
}

fn low_cut_off(hz: f64) -> bool {
    hz <= f64::from(LOW_CUT_OFF_HZ) + 0.01
}

/// The graph: spectrum after the layers (with the EQ's curve applied,
/// so it shows what goes on to the output), the curve, the handles.
/// Drag a handle to move its band, wheel over a bell for its width,
/// double-click to flatten it.
#[allow(clippy::too_many_lines)]
pub(crate) fn graph_panel(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    view: &View,
    r: Rect,
    accent: Color32,
) {
    let p = ui.painter();
    panel(p, r);
    let ty = r.top() + 22.0;
    crate::assist::switch(ui, st, r, accent);
    let on = params.get_plain(ParamId::FxOn) >= 0.5 && params.get_plain(ParamId::EqOn) >= 0.5;
    #[allow(clippy::cast_possible_truncation)]
    let sr = if view.sample_rate > 0.0 {
        view.sample_rate as f32
    } else {
        48_000.0
    };
    let s = settings(params);
    let curve = EqCurve::new(&s, sr);

    // The selected band, in words.
    let sel = st.eq.band.min(EQ_BANDS - 1);
    let b = BANDS[sel];
    let hz = params.get_plain(b.freq);
    let reading = if !on {
        "Off".to_owned()
    } else if b.gain.is_none() && low_cut_off(hz) {
        format!("{} · {} · Off", sel + 1, b.name)
    } else {
        // Short: the switch on the left leaves room for about 30 letters.
        let mut t = format!("{} · {} Hz", sel + 1, hz_text(hz));
        if let Some(g) = b.gain {
            t += &format!(" · {:+.1} dB", params.get_plain(g));
        }
        if let Some(q) = b.q {
            t += &format!(" · Q {:.1}", params.get_plain(q));
        }
        t
    };
    crate::paint::text_fit(
        p,
        pos2(r.right() - space::L, ty),
        Align2::RIGHT_CENTER,
        &reading,
        size::CAPTION,
        if on {
            color::TEXT_DIM
        } else {
            color::TEXT_FAINT
        },
        r.width() - 2.0 * space::L - 150.0 - space::M,
    );

    let plot = Rect::from_min_max(
        pos2(r.left() + space::L, r.top() + 44.0),
        pos2(r.right() - space::L, r.bottom() - 30.0),
    );
    well(p, plot);
    let x_of = |hz: f32| {
        plot.left() + plot.width() * (hz.clamp(F_MIN, F_MAX) / F_MIN).ln() / (F_MAX / F_MIN).ln()
    };
    let hz_at =
        |x: f32| F_MIN * (F_MAX / F_MIN).powf(((x - plot.left()) / plot.width()).clamp(0.0, 1.0));
    let y_of = |db: f32| plot.center().y - 0.5 * plot.height() * (db / GAIN_SPAN).clamp(-1.0, 1.0);
    let db_at = |y: f32| (plot.center().y - y) / (0.5 * plot.height()) * GAIN_SPAN;

    // Grid: decades and their 2 and 5, every 6 dB.
    for (hz, name) in [
        (50.0, ""),
        (100.0, "100"),
        (200.0, ""),
        (500.0, ""),
        (1000.0, "1k"),
        (2000.0, ""),
        (5000.0, ""),
        (10_000.0, "10k"),
    ] {
        let x = x_of(hz);
        let strong = !name.is_empty();
        p.line_segment(
            [pos2(x, plot.top() + 1.0), pos2(x, plot.bottom() - 1.0)],
            Stroke::new(
                1.0_f32,
                if strong {
                    color::GRID_STRONG
                } else {
                    color::GRID
                },
            ),
        );
        if strong {
            text(
                p,
                pos2(x, plot.bottom() + 13.0),
                Align2::CENTER_CENTER,
                name,
                size::CAPTION,
                color::TEXT_FAINT,
            );
        }
    }
    text(
        p,
        pos2(plot.right(), plot.bottom() + 13.0),
        Align2::RIGHT_CENTER,
        "Hz",
        size::CAPTION,
        color::TEXT_FAINT,
    );
    for db in [-12.0_f32, -6.0, 0.0, 6.0, 12.0] {
        let y = y_of(db);
        p.line_segment(
            [pos2(plot.left() + 1.0, y), pos2(plot.right() - 1.0, y)],
            Stroke::new(
                1.0_f32,
                if db == 0.0 {
                    color::GRID_STRONG
                } else {
                    color::GRID
                },
            ),
        );
        if db.abs() > 11.0 {
            text(
                p,
                pos2(plot.left() + 6.0, y),
                Align2::LEFT_CENTER,
                &format!("{db:+.0}"),
                size::CAPTION,
                color::TEXT_FAINT,
            );
        }
    }

    let clip = p.with_clip_rect(plot.shrink(1.0));
    // Spectrum of the transformed sound, as the EQ leaves it.
    if let Some(taps) = st.eq.taps.clone() {
        st.eq.spectrum.update(&taps, sr);
        let mut mesh = egui::Mesh::default();
        let mut line = Vec::with_capacity(POINTS);
        let fill = accent.gamma_multiply(0.16);
        for (i, db) in st.eq.spectrum.db.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let hz = point_hz(i as f32);
            let db = db + if on { curve.gain_db(hz) } else { 0.0 };
            let t = ((db - FLOOR_DB) / (TOP_DB - FLOOR_DB)).clamp(0.0, 1.0);
            let at = pos2(x_of(hz), plot.bottom() - t * plot.height());
            #[allow(clippy::cast_possible_truncation)]
            let k = mesh.vertices.len() as u32;
            mesh.colored_vertex(at, fill);
            mesh.colored_vertex(pos2(at.x, plot.bottom()), fill);
            if i > 0 {
                mesh.add_triangle(k - 2, k - 1, k);
                mesh.add_triangle(k - 1, k, k + 1);
            }
            line.push(at);
        }
        clip.add(egui::Shape::mesh(mesh));
        clip.add(egui::Shape::line(
            line,
            Stroke::new(1.0_f32, accent.gamma_multiply(0.45)),
        ));
    }

    // The curve.
    let curve_col = if on { accent } else { color::TEXT_FAINT };
    let steps = 160;
    let mut mesh = egui::Mesh::default();
    let mut line = Vec::with_capacity(steps + 1);
    let tint = curve_col.gamma_multiply(0.10);
    for i in 0..=steps {
        #[allow(clippy::cast_precision_loss)]
        let x = plot.left() + plot.width() * i as f32 / steps as f32;
        let at = pos2(x, y_of(curve.gain_db(hz_at(x))));
        #[allow(clippy::cast_possible_truncation)]
        let k = mesh.vertices.len() as u32;
        mesh.colored_vertex(at, tint);
        mesh.colored_vertex(pos2(x, y_of(0.0)), tint);
        if i > 0 {
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k, k + 1);
        }
        line.push(at);
    }
    clip.add(egui::Shape::mesh(mesh));
    clip.add(egui::Shape::line(line, Stroke::new(2.0_f32, curve_col)));

    // Handles, the selected one last (on top).
    let order = (0..EQ_BANDS).filter(|i| *i != sel).chain([sel]);
    for i in order {
        let b = BANDS[i];
        let kind = b.freq.spec().kind;
        let hz = params.get_plain(b.freq);
        #[allow(clippy::cast_possible_truncation)]
        let gain = b.gain.map_or(0.0, |g| params.get_plain(g) as f32);
        #[allow(clippy::cast_possible_truncation)]
        let at = pos2(x_of(hz as f32), y_of(gain));
        let hit = Rect::from_center_size(at, vec2(2.0 * HANDLE_R + 4.0, 2.0 * HANDLE_R + 4.0));
        let resp = ui.interact(
            hit,
            ui.id().with(("quack-eq-handle", i)),
            Sense::click_and_drag(),
        );
        if resp.drag_started() {
            st.eq.drag = Some(i);
            st.eq.band = i;
            st.fx_tab = FxTab::Eq;
            params.begin_edit(b.freq);
            if let Some(g) = b.gain {
                params.begin_edit(g);
            }
        }
        if resp.dragged()
            && st.eq.drag == Some(i)
            && let Some(now) = resp.interact_pointer_pos()
        {
            let mut f = f64::from(hz_at(now.x)).clamp(kind.min(), kind.max());
            if b.gain.is_none() && now.x <= plot.left() + 6.0 {
                // The low cut pushed to the left edge: off.
                f = f64::from(LOW_CUT_OFF_HZ);
            }
            if (f - hz).abs() > 1e-6 {
                params.set_plain(b.freq, f);
            }
            if let Some(g) = b.gain {
                let gk = g.spec().kind;
                let mut db = f64::from(db_at(now.y)).clamp(gk.min(), gk.max());
                if db.abs() < 0.25 {
                    db = 0.0;
                }
                if (db - f64::from(gain)).abs() > 1e-6 {
                    params.set_plain(g, db);
                }
            }
        }
        if resp.drag_stopped() && st.eq.drag == Some(i) {
            st.eq.drag = None;
            params.end_edit(b.freq);
            if let Some(g) = b.gain {
                params.end_edit(g);
            }
        }
        if resp.clicked() {
            st.eq.band = i;
            st.fx_tab = FxTab::Eq;
        }
        if resp.double_clicked() {
            match b.gain {
                Some(g) => set_once(params, g, 0.0),
                None => set_once(params, b.freq, f64::from(LOW_CUT_OFF_HZ)),
            }
        } else if resp.hovered()
            && st.eq.drag.is_none()
            && let Some(q) = b.q
        {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                // Wheel up: a narrower bell.
                let now = params.get_plain(q);
                set_once(params, q, now * if scroll > 0.0 { 1.1 } else { 1.0 / 1.1 });
            }
        }
        if resp.hovered() || st.eq.drag == Some(i) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        let lit = i == sel;
        let flat = b.gain.map_or(low_cut_off(hz), |_| gain.abs() < 0.01);
        let ring = if !on {
            color::TEXT_FAINT
        } else if lit || !flat {
            accent
        } else {
            accent.gamma_multiply(0.55)
        };
        p.circle(
            at,
            HANDLE_R,
            if lit {
                accent.gamma_multiply(if on { 0.35 } else { 0.15 })
            } else {
                color::PANEL
            },
            Stroke::new(if lit || resp.hovered() { 2.0_f32 } else { 1.5 }, ring),
        );
        value(
            p,
            at,
            Align2::CENTER_CENTER,
            &format!("{}", i + 1),
            size::CAPTION,
            if lit && on { color::TEXT } else { ring },
        );
    }
}

/// The EQ tab: which band the knobs show, its frequency, gain and
/// width, then the exciter.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tab(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    r: Rect,
    y1: f32,
    y2: f32,
    live: Color32,
    dim: Color32,
) {
    let p = ui.painter();
    let inner_w = r.width() - 2.0 * space::L;
    let labels: Vec<String> = BANDS
        .iter()
        .enumerate()
        .map(|(i, b)| format!("{} {}", i + 1, b.name))
        .collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let seg = Rect::from_min_size(pos2(r.left() + space::L, y1 - 14.0), vec2(inner_w, 28.0));
    let mut band = st.eq.band.min(EQ_BANDS - 1);
    controls::tabs(ui, "eq-band", seg, &labels, &mut band, live);
    st.eq.band = band;
    let b = BANDS[band];

    let hz = |v: f64| hz_text(v);
    let cut = |v: f64| {
        if low_cut_off(v) {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let sdb = |v: f64| format!("{v:+.1}");
    let q = |v: f64| format!("{v:.1}");
    let off = |v: f64| {
        if v < 0.5 {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let kw = inner_w / 5.0;
    #[allow(clippy::cast_precision_loss)]
    let slot = |i: usize| pos2(r.left() + space::L + kw * (i as f32 + 0.5), y2);
    let knob = |ui: &egui::Ui,
                st: &mut UiState,
                id: ParamId,
                at: usize,
                name: &str,
                fmt: &dyn Fn(f64) -> String,
                acc: Color32,
                log: bool| {
        controls::knob_tapered(
            ui,
            params,
            st,
            id,
            slot(at),
            54.0,
            id.spec().kind.min(),
            acc,
            fmt,
            name,
            log,
        );
    };
    if b.gain.is_none() {
        knob(ui, st, b.freq, 0, "Low Cut", &cut, live, true);
    } else {
        knob(ui, st, b.freq, 0, "Freq", &hz, live, true);
    }
    if let Some(g) = b.gain {
        knob(ui, st, g, 1, "Gain", &sdb, live, false);
    }
    if let Some(id) = b.q {
        let flat = b.gain.is_some_and(|g| params.get_plain(g).abs() < 0.01);
        knob(ui, st, id, 2, "Q", &q, if flat { dim } else { live }, true);
    }
    // The exciter, apart on the right.
    let x = r.left() + space::L + kw * 3.0;
    p.line_segment(
        [pos2(x, y2 - 34.0), pos2(x, y2 + 44.0)],
        Stroke::new(1.0_f32, color::BORDER),
    );
    let exciting = params.get_plain(ParamId::ExciterAmount) >= 0.5;
    knob(
        ui,
        st,
        ParamId::ExciterAmount,
        3,
        "Exciter",
        &off,
        live,
        false,
    );
    knob(
        ui,
        st,
        ParamId::ExciterFreq,
        4,
        "From",
        &hz,
        if exciting { live } else { dim },
        true,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_finds_a_sine_where_it_is() {
        let taps = Taps::default();
        #[allow(clippy::cast_precision_loss)]
        for i in 0..8192 {
            taps.layers
                .push(0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin());
        }
        let mut s = Spectrum::default();
        for n in 0..40 {
            // New samples each frame, or the picture falls away.
            taps.layers.push(0.0);
            let _ = n;
            s.update(&taps, 48_000.0);
        }
        let (peak, db) =
            s.db.iter().enumerate().fold(
                (0, -300.0_f32),
                |a, (i, d)| if *d > a.1 { (i, *d) } else { a },
            );
        #[allow(clippy::cast_precision_loss)]
        let hz = point_hz(peak as f32);
        assert!((hz / 1000.0).log2().abs() < 0.08, "{hz}");
        // A sine at half scale: -6 dB, no tilt at 1 kHz.
        assert!((db + 6.0).abs() < 1.5, "{db}");
    }
}
