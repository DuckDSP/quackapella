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
//! their DSP steps; step 8 adds the preset bar, the macro strip, the
//! right-click menu (MIDI learn, macro depths) and zoom ([`perform`]).

mod brand;
mod controls;
pub mod library;
mod paint;
pub mod perform;
pub mod theme;

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use quack_dsp::autotune::{Scale, Source, scale_mask};
use quack_dsp::chain::{Chain, Routing, Stage};
use quack_dsp::creative::{GlitchMode, GlitchRate, glitch_step};
use quack_dsp::harmony::{
    HarmonyMode, interval_degree, interval_quality, interval_semitones, interval_short,
};
use quack_dsp::latency::VoiceRange;
use quack_dsp::params::NOTE_PARAMS;
use quack_dsp::vocoder::{Carrier, VocoderMode, bands_from_index};
use quack_dsp::{ParamId, VoiceMode};

use paint::{panel, spaced, text, value, well};
use theme::{color, radius, size, space};

/// Editor size in points at 100 % zoom (the zoom scales it, with the
/// AUv2 resize patch in `vendor/`).
pub const BASE_W: u32 = 1040;
pub const BASE_H: u32 = 904;

/// Parameter access in plain values, implemented over the host by the
/// plugin and by a plain array in tests.
pub trait ParamAccess {
    fn get_plain(&self, id: ParamId) -> f64;
    fn begin_edit(&self, id: ParamId);
    fn set_plain(&self, id: ParamId, plain: f64);
    fn end_edit(&self, id: ParamId);
    /// Macros, MIDI learn and the loaded preset (step 8). `None`: the
    /// editor shows no macro assignments or learn menus.
    fn control(&self) -> Option<&quack_dsp::control::ControlState> {
        None
    }
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
    /// A frozen grain is sounding (Creative layer).
    pub frozen: bool,
    /// Notes the harmony voices sing, bit n = note n.
    pub harmony_notes: u128,
    /// Notes the vocoder's carrier plays, bit n = note n.
    pub vocoder_notes: u128,
    /// Notes the MIDI output plays, bit n = note n.
    pub midi_out_notes: u128,
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
    /// Layer shown in the big panel.
    page: Page,
    /// Preset browser, values of this frame.
    presets: perform::PresetUi,
    /// Perform row lists open, and where they hang from.
    keys_menu: bool,
    channel_menu: bool,
    source_menu: bool,
    keys_rect: egui::Rect,
    channel_rect: egui::Rect,
    source_rect: egui::Rect,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            drag: None,
            history: vec![Sample::default(); HISTORY_LEN],
            head: 0,
            centre: 60.0,
            scale_menu: false,
            page: Page::Voice,
            presets: perform::PresetUi::default(),
            keys_menu: false,
            channel_menu: false,
            source_menu: false,
            keys_rect: egui::Rect::NOTHING,
            channel_rect: egui::Rect::NOTHING,
            source_rect: egui::Rect::NOTHING,
        }
    }
}

/// The layers, in the strip's order. Each has a page in the big panel
/// (the voice's is the pitch graph).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Voice,
    Creative,
    Harmony,
    Vocoder,
}

impl Page {
    pub const ALL: [Self; 4] = [Self::Voice, Self::Creative, Self::Harmony, Self::Vocoder];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Voice => "Voice",
            Self::Creative => "Creative",
            Self::Harmony => "Harmony",
            Self::Vocoder => "Vocoder",
        }
    }

    /// The layer's switch (the voice is always on).
    #[must_use]
    pub const fn switch(self) -> Option<ParamId> {
        match self {
            Self::Voice => None,
            Self::Creative => Some(ParamId::CreativeOn),
            Self::Harmony => Some(ParamId::HarmonyOn),
            Self::Vocoder => Some(ParamId::VocoderOn),
        }
    }

    /// The layer's level.
    #[must_use]
    pub const fn level(self) -> ParamId {
        match self {
            Self::Voice => ParamId::Lead,
            Self::Creative => ParamId::CreativeLevel,
            Self::Harmony => ParamId::HarmonyLevel,
            Self::Vocoder => ParamId::VocoderLevel,
        }
    }
}

impl UiState {
    /// Show a layer's page in the big panel.
    pub fn set_page(&mut self, page: Page) {
        self.page = page;
    }

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
    }
}

/// Accent of a layer: the voice in its correction style's.
#[must_use]
pub fn page_accent(page: Page, style: VoiceMode) -> Color32 {
    match page {
        Page::Voice => mode_accent(style),
        Page::Creative => color::CREATIVE,
        Page::Harmony => color::SYNTH,
        Page::Vocoder => color::VOCODER,
    }
}

/// Whether a layer sounds (its switch on).
fn layer_on(params: &dyn ParamAccess, page: Page) -> bool {
    page.switch().is_none_or(|id| params.get_plain(id) >= 0.5)
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

    perform::refresh(params, st);

    // Header: wordmark, presets, latency, bypass.
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

    // Layers strip: each layer's switch, page and level; then the
    // correction style, the voice range and the latency switch.
    let mode_r = Rect::from_min_size(
        pos2(inner.left(), inner.top() + 48.0),
        vec2(inner.width(), 82.0),
    );
    layers_panel(ui, params, st, mode_r, mode);

    // Presets between the wordmark and the latency (drawn after the
    // mode panel so the browser opens over it).
    let bar_left = inner.left() + 230.0;
    let bar_r = Rect::from_min_max(
        pos2(bar_left, head_y - 15.0),
        pos2(lat_caption.left() - space::XL, head_y + 15.0),
    );
    let bar_r = Rect::from_min_size(bar_r.min, vec2(bar_r.width().min(440.0), 30.0));

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
    let page_acc = page_accent(st.page, mode);
    match st.page {
        Page::Voice => pitch_graph(&p, graph_r, view, st, accent),
        Page::Creative => creative_panel(ui, params, st, view, graph_r, page_acc),
        Page::Harmony => harmony_panel(ui, params, st, view, graph_r, page_acc),
        Page::Vocoder => vocoder_panel(ui, params, st, view, graph_r, page_acc),
    }

    // Bottom row: voice, level, MIDI.
    let row_top = tune_r.bottom() + space::M;
    let foot_h = 26.0;
    let macro_h = 92.0;
    let perform_h = 72.0;
    let row_h = inner.bottom() - foot_h - macro_h - perform_h - 2.0 * space::M - row_top;
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

    // Perform row: snapshots and morph under the voice and level,
    // MIDI out under MIDI in.
    let perform_top = midi_r.bottom() + space::M;
    let snap_r = Rect::from_min_max(
        pos2(inner.left(), perform_top),
        pos2(level_r.right(), perform_top + perform_h),
    );
    perform::snapshot_panel(ui, params, st, snap_r);
    let out_r = Rect::from_min_max(
        pos2(midi_r.left(), perform_top),
        pos2(inner.right(), perform_top + perform_h),
    );
    perform::midi_out_panel(ui, params, st, view, out_r, accent);

    // Macros, program change, zoom.
    let macro_r = Rect::from_min_size(
        pos2(inner.left(), out_r.bottom() + space::M),
        vec2(inner.width(), macro_h),
    );
    perform::macro_panel(ui, params, st, macro_r, accent);
    perform::perform_menus(ui, params, st, accent);

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
    perform::learn_status(ui, params, pos2(inner.center().x, mark.center().y));

    perform::preset_bar(ui, params, st, bar_r);

    // Safety net: a drag cut short (window lost focus) ends its edit.
    if st.drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
        st.end_open_edits(params);
    }
}

/// The layers strip: one cell per layer (switch, name opening its
/// page, level), then the correction style, the voice range and the
/// latency switch on the right.
fn layers_panel(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    r: Rect,
    style: VoiceMode,
) {
    let p = ui.painter();
    panel(p, r);
    let row1 = r.top() + 9.0;
    let row2 = r.top() + 47.0;
    // Right block: voice range and latency, then the style.
    let right_w = 290.0;
    let voice_r = Rect::from_min_size(pos2(r.right() - space::L - 150.0, row1), vec2(150.0, 28.0));
    let range = VoiceRange::from_index(params.get_plain(ParamId::Voice));
    controls::stepper(
        ui,
        params,
        ParamId::Voice,
        voice_r,
        range.name(),
        color::QUACK,
    );
    let low_r = Rect::from_min_size(
        pos2(r.right() - space::L - right_w, row1),
        vec2(right_w - 150.0 - space::S, 28.0),
    );
    controls::toggle(
        ui,
        params,
        ParamId::LowLatency,
        low_r,
        "Low Latency",
        color::QUACK,
    );
    let style_r = Rect::from_min_size(pos2(low_r.left() + 52.0, row2), vec2(right_w - 52.0, 28.0));
    text(
        p,
        pos2(low_r.left(), style_r.center().y),
        Align2::LEFT_CENTER,
        "Style",
        size::CAPTION,
        color::TEXT_FAINT,
    );
    let names: Vec<&str> = VoiceMode::ALL.iter().map(|m| m.name()).collect();
    let accents: Vec<Color32> = VoiceMode::ALL.iter().map(|m| mode_accent(*m)).collect();
    controls::segmented(ui, params, ParamId::Mode, style_r, &names, &accents);

    // Routing (parallel or series), then the layer cells, in chain
    // order in series with a chevron between them (click: swap).
    let routing_w = 84.0;
    let route_x = r.left() + space::L;
    routing_switch(
        ui,
        params,
        pos2(route_x, row1),
        routing_w,
        row2 + 28.0 - row1,
    );
    let series = Routing::from_index(params.get_plain(ParamId::Routing)) == Routing::Series;
    let chain = Chain::from_index(params.get_plain(ParamId::Chain));
    let order = layer_order(series, chain);
    let left = route_x + routing_w + space::M;
    let right = low_r.left() - space::XL;
    let gap = 18.0;
    #[allow(clippy::cast_precision_loss)]
    let w = (right - left - gap * (order.len() - 1) as f32) / order.len() as f32;
    for (i, page) in order.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let x = left + (w + gap) * i as f32;
        let acc = page_accent(page, style);
        let on = layer_on(params, page);
        let tab = Rect::from_min_size(pos2(x, row1), vec2(w, 28.0));
        layer_tab(ui, params, st, tab, page, on, acc);
        let fader = Rect::from_min_size(pos2(x, row2), vec2(w, 28.0));
        let label = if page == Page::Voice { "Lead" } else { "Level" };
        controls::fader(
            ui,
            params,
            st,
            page.level(),
            fader,
            if on { acc } else { acc.gamma_multiply(0.35) },
            label,
        );
        if series && i > 0 {
            let g = Rect::from_min_size(pos2(x - gap, row1), vec2(gap, row2 + 28.0 - row1));
            chain_link(ui, params, g, chain, i - 1);
        }
    }
}

/// The layers in the order they play: parallel, the fixed order; in
/// series, the voice then the chain.
fn layer_order(series: bool, chain: Chain) -> [Page; 4] {
    if !series {
        return Page::ALL;
    }
    let page = |s: Stage| match s {
        Stage::Creative => Page::Creative,
        Stage::Harmony => Page::Harmony,
        Stage::Vocoder => Page::Vocoder,
    };
    [
        Page::Voice,
        page(chain.0[0]),
        page(chain.0[1]),
        page(chain.0[2]),
    ]
}

/// Parallel / Series, stacked.
fn routing_switch(ui: &egui::Ui, params: &dyn ParamAccess, at: egui::Pos2, w: f32, h: f32) {
    let p = ui.painter();
    let r = Rect::from_min_size(at, vec2(w, h));
    paint::rounded(p, r, radius::CONTROL, color::WELL, color::BORDER);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lit = params.get_plain(ParamId::Routing).round() as usize;
    for (i, routing) in Routing::ALL.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let seg = Rect::from_min_size(
            pos2(r.left(), r.top() + h / 2.0 * i as f32),
            vec2(w, h / 2.0),
        );
        let resp = ui.interact(seg, ui.id().with(("quack-routing", i)), Sense::click());
        if resp.clicked() && i != lit {
            #[allow(clippy::cast_precision_loss)]
            set_once(params, ParamId::Routing, i as f64);
        }
        crate::perform::attach(ui, &resp, params, ParamId::Routing);
        let resp = resp.on_hover_text(match routing {
            Routing::Parallel => "Every layer is built from the voice, then mixed",
            Routing::Series => "Each module processes what comes before it",
        });
        if i == lit {
            paint::rounded(
                p,
                seg.shrink(2.0),
                radius::CONTROL - 1,
                color::QUACK.gamma_multiply(0.22),
                color::QUACK.gamma_multiply(0.8),
            );
        } else if resp.hovered() {
            paint::rounded(
                p,
                seg.shrink(2.0),
                radius::CONTROL - 1,
                color::RAISED,
                color::RAISED,
            );
        }
        let c = if i == lit {
            color::QUACK
        } else {
            color::TEXT_DIM
        };
        text(
            p,
            seg.center(),
            Align2::CENTER_CENTER,
            routing.name(),
            size::CAPTION,
            c,
        );
    }
}

/// The chevron between two layers in series. Between two modules it
/// swaps them (`slot`: the first one's place in the chain); after the
/// voice it only shows the direction.
fn chain_link(ui: &egui::Ui, params: &dyn ParamAccess, r: Rect, chain: Chain, slot: usize) {
    let p = ui.painter();
    let swappable = slot > 0;
    let hovered = if swappable {
        let resp = ui
            .interact(r, ui.id().with(("quack-chain", slot)), Sense::click())
            .on_hover_text("Swap these two modules");
        if resp.clicked() {
            #[allow(clippy::cast_precision_loss)]
            set_once(
                params,
                ParamId::Chain,
                chain.swapped(slot - 1).index() as f64,
            );
        }
        resp.hovered()
    } else {
        false
    };
    let c = if hovered {
        color::QUACK
    } else {
        color::TEXT_FAINT
    };
    let mid = r.center();
    let a = 4.0;
    let stroke = egui::Stroke::new(if hovered { 2.0_f32 } else { 1.5 }, c);
    for dy in [-19.0, 19.0] {
        let y = mid.y + dy;
        p.line_segment(
            [pos2(mid.x - a / 2.0, y - a), pos2(mid.x + a / 2.0, y)],
            stroke,
        );
        p.line_segment(
            [pos2(mid.x + a / 2.0, y), pos2(mid.x - a / 2.0, y + a)],
            stroke,
        );
    }
    if swappable && hovered {
        // Arrows both ways: a swap.
        p.line_segment([pos2(mid.x - 5.0, mid.y), pos2(mid.x + 5.0, mid.y)], stroke);
    }
}

/// A layer's tab: the switch (a dot) on the left, the name opening the
/// layer's page. The voice has no switch.
fn layer_tab(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    r: Rect,
    page: Page,
    on: bool,
    accent: Color32,
) {
    let p = ui.painter();
    let shown = st.page == page;
    let power = Rect::from_min_size(r.min, vec2(r.height(), r.height()));
    let name_r = Rect::from_min_max(pos2(power.right(), r.top()), r.max);
    let resp = ui.interact(
        name_r,
        ui.id().with(("quack-layer-tab", page as u8)),
        Sense::click(),
    );
    if resp.clicked() {
        st.page = page;
    }
    if shown {
        paint::rounded(
            p,
            r,
            radius::CONTROL,
            accent.gamma_multiply(0.22),
            accent.gamma_multiply(0.8),
        );
    } else {
        paint::rounded(
            p,
            r,
            radius::CONTROL,
            if resp.hovered() {
                color::BORDER
            } else {
                color::RAISED
            },
            color::BORDER,
        );
    }
    text(
        p,
        name_r.center() - vec2(power.width() * 0.25, 0.0),
        Align2::CENTER_CENTER,
        page.name(),
        size::BODY,
        if shown { accent } else { color::TEXT_DIM },
    );
    // The switch: a lit dot when the layer sounds.
    let dot = power.center() + vec2(2.0, 0.0);
    if let Some(id) = page.switch() {
        let presp = ui.interact(
            power,
            ui.id().with(("quack-layer-on", page as u8)),
            Sense::click(),
        );
        if presp.clicked() {
            set_once(params, id, if on { 0.0 } else { 1.0 });
        }
        crate::perform::attach(ui, &presp, params, id);
        let ring = if presp.hovered() {
            color::TEXT_DIM
        } else {
            color::TEXT_FAINT
        };
        p.circle_stroke(
            dot,
            6.5,
            egui::Stroke::new(1.5_f32, if on { accent } else { ring }),
        );
        if on {
            p.circle_filled(dot, 3.5, accent);
        }
    } else {
        p.circle_filled(dot, 3.5, accent);
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

/// The creative layer: octave layers, formant follow, robot, freeze,
/// glitch, all on the lead itself.
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
    layer_switch(ui, params, ParamId::CreativeOn, r, "Creative", accent);
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

/// The harmony layer: switch, source, intervals and what makes the
/// voices sound like several singers.
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
    layer_switch(ui, params, ParamId::HarmonyOn, r, "Harmony", accent);
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
        pos2(
            r.left() + space::L + LAYER_SWITCH_W + space::M,
            r.top() + 22.0,
        ),
        Align2::LEFT_CENTER,
        &status,
        size::VALUE,
        if sounding.is_empty() {
            color::TEXT_FAINT
        } else {
            accent
        },
    );

    let knob_d = 64.0;
    let kw = (r.width() - 2.0 * space::L) / 6.0;
    let row1 = r.top() + 124.0;
    let cap_y = row1 - knob_d * 0.5 - 16.0;
    let intervals_on = mode != HarmonyMode::Midi;
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
        HarmonyMode::Off | HarmonyMode::Midi => "Every held MIDI note is a voice".to_owned(),
        HarmonyMode::Fixed => "Exact intervals from the lead (semitones)".to_owned(),
        HarmonyMode::Scale => format!("Semitones from the lead, kept in {key} {}", scale.name()),
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
        accent,
    );
    let interval = |v: f64| interval_short(interval_semitones(v));
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
        let semis = interval_semitones(params.get_plain(id));
        let label = match mode {
            _ if semis == 0 => format!("Voice {}", i + 1),
            HarmonyMode::Fixed => interval_quality(semis),
            HarmonyMode::Scale | HarmonyMode::Chord => interval_degree(semis),
            _ => format!("Voice {}", i + 1),
        };
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
        // The voice's own level, under it.
        let fr = Rect::from_center_size(
            pos2(cx, row1 + knob_d * 0.5 + 38.0),
            vec2(kw - space::S, 22.0),
        );
        let level = quack_dsp::params::HARMONY_LEVEL_PARAMS[i];
        let live = intervals_on && semis != 0;
        controls::fader(
            ui,
            params,
            st,
            level,
            fr,
            if live { accent } else { dim },
            "",
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
    // Close voicing and latch, for the played notes (MIDI and Chord).
    let tog_x = r.left() + space::L + kw * 4.0 + space::M;
    let tog_w = kw * 2.0 - 2.0 * space::M;
    let close_r = Rect::from_min_size(pos2(tog_x, row1 - 34.0), vec2(tog_w, 28.0));
    controls::toggle(
        ui,
        params,
        ParamId::CloseVoicing,
        close_r,
        "Close Voicing",
        accent,
    );
    let latch_r = Rect::from_min_size(pos2(tog_x, close_r.bottom() + space::S), vec2(tog_w, 28.0));
    controls::toggle(ui, params, ParamId::Latch, latch_r, "Latch", accent);
    text(
        p,
        pos2(latch_r.center().x, latch_r.bottom() + 16.0),
        Align2::CENTER_CENTER,
        match mode {
            HarmonyMode::Midi => "The sustain pedal holds notes too",
            HarmonyMode::Chord => "Latch or pedal hold the chord",
            _ => "MIDI and Chord sources",
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

/// Width of a layer's switch in its page's title.
const LAYER_SWITCH_W: f32 = 110.0;

/// A layer's switch as its page's title (top left of `r`).
fn layer_switch(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    id: ParamId,
    r: Rect,
    label: &str,
    accent: Color32,
) {
    let sw = Rect::from_min_size(
        pos2(r.left() + space::L, r.top() + 8.0),
        vec2(LAYER_SWITCH_W, 28.0),
    );
    controls::toggle(ui, params, id, sw, label, accent);
}

/// The vocoder layer, vocoder or talkbox: the voice shapes the internal
/// carrier synth (or the side-chain).
fn vocoder_panel(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    view: &View,
    r: Rect,
    accent: Color32,
) {
    let p = ui.painter();
    panel(p, r);
    layer_switch(ui, params, ParamId::VocoderOn, r, "Vocoder", accent);
    let layer = params.get_plain(ParamId::VocoderOn) >= 0.5;
    let mode = VocoderMode::from_index(params.get_plain(ParamId::Vocoder));
    let seg = Rect::from_min_size(
        pos2(r.right() - space::L - 240.0, r.top() + 8.0),
        vec2(240.0, 28.0),
    );
    let names: Vec<&str> = VocoderMode::ALL.iter().map(|m| m.name()).collect();
    controls::segmented(ui, params, ParamId::Vocoder, seg, &names, &[accent]);
    // Carrier choice, and what plays it.
    let carrier = Carrier::from_index(params.get_plain(ParamId::Carrier));
    let car_r = Rect::from_min_size(
        pos2(r.left() + space::L, r.top() + 48.0),
        vec2(r.width() - 2.0 * space::L, 26.0),
    );
    let cnames: Vec<&str> = Carrier::ALL.iter().map(|c| c.name()).collect();
    controls::segmented(ui, params, ParamId::Carrier, car_r, &cnames, &[accent]);
    let playing: Vec<String> = (0..128u8)
        .filter(|n| view.vocoder_notes & (1u128 << n) != 0)
        .map(note_name)
        .collect();
    let follow = params.get_plain(ParamId::CarrierFollow) >= 0.5;
    let shown = playing
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let more = if playing.len() > 3 { " …" } else { "" };
    let status = match (mode, carrier) {
        _ if !layer => "Off".to_owned(),
        (_, Carrier::Sidechain) => "Side-chain".to_owned(),
        _ if !playing.is_empty() && view.held_notes == 0 && follow => {
            format!("Follow · {shown}{more}")
        }
        _ if !playing.is_empty() => format!("{shown}{more}"),
        _ if follow => "Sing or play".to_owned(),
        _ => "Play MIDI".to_owned(),
    };
    text(
        p,
        pos2(
            r.left() + space::L + LAYER_SWITCH_W + space::M,
            r.top() + 22.0,
        ),
        Align2::LEFT_CENTER,
        &status,
        size::CAPTION,
        if playing.is_empty() || !layer {
            color::TEXT_FAINT
        } else {
            accent
        },
    );

    let knob_d = 54.0;
    let off = |v: f64| {
        if v < 0.5 {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let ms = |v: f64| format!("{v:.0}");
    let st_fmt = |v: f64| format!("{v:+.1}");
    let bands = |v: f64| format!("{}", bands_from_index(v));
    #[allow(clippy::cast_possible_truncation)]
    let octave = |v: f64| format!("{:+}", v.round() as i32 - 2);
    let vocoder_only = mode != VocoderMode::Talkbox;
    let pitched = !matches!(carrier, Carrier::Noise | Carrier::Sidechain);
    let dim = accent.gamma_multiply(0.35);
    type Knob<'a> = (ParamId, &'a str, &'a dyn Fn(f64) -> String, bool);
    let row1: [Knob; 6] = [
        (ParamId::BendRange, "Bend", &st_fmt, pitched),
        (ParamId::Bands, "Bands", &bands, vocoder_only),
        (ParamId::Response, "Response", &ms, vocoder_only),
        (ParamId::BandShift, "Shift", &st_fmt, true),
        (ParamId::Sibilance, "Sibilance", &off, true),
        (ParamId::VocoderWidth, "Width", &off, true),
    ];
    let synth = carrier != Carrier::Sidechain;
    let row2: [Knob; 6] = [
        (ParamId::CarrierOctave, "Octave", &octave, pitched),
        (ParamId::Detune, "Detune", &off, pitched),
        (ParamId::CarrierNoise, "Noise", &off, pitched),
        (ParamId::CarrierRelease, "Release", &ms, synth),
        (ParamId::CarrierGlide, "Glide", &off, pitched),
        (ParamId::VocoderLofi, "Lo-Fi", &off, true),
    ];
    let kw = (r.width() - 2.0 * space::L) / 6.0;
    let y1 = r.top() + 156.0;
    let y2 = r.bottom() - 64.0;
    for (row, y) in [(&row1[..], y1), (&row2[..], y2)] {
        for (i, (id, label, fmt, live)) in row.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let cx = r.left() + space::L + kw * (i as f32 + 0.5);
            controls::knob_named(
                ui,
                params,
                st,
                *id,
                pos2(cx, y),
                knob_d,
                id.spec().kind.min(),
                if *live { accent } else { dim },
                *fmt,
                label,
            );
        }
    }
    // Follow (the carrier sings the tuned note without MIDI) and its
    // chord, Hold (the timbre frozen on the carrier).
    let row = car_r.bottom() + space::S;
    let fr = Rect::from_min_size(pos2(car_r.left(), row), vec2(80.0, 26.0));
    controls::toggle(ui, params, ParamId::CarrierFollow, fr, "Follow", accent);
    let hr = Rect::from_min_size(pos2(car_r.right() - 70.0, row), vec2(70.0, 26.0));
    controls::toggle(ui, params, ParamId::VocoderHold, hr, "Hold", accent);
    let cr = Rect::from_min_max(
        pos2(fr.right() + space::S, row),
        pos2(hr.left() - space::S, row + 26.0),
    );
    let chords = ["One", "Oct", "5th", "Maj", "Min", "Scale"];
    controls::segmented(
        ui,
        params,
        ParamId::FollowChord,
        cr,
        &chords,
        &[if follow && synth { accent } else { dim }],
    );
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
        for (mode, page) in Page::ALL.into_iter().enumerate() {
            let mode = u8::try_from(mode).unwrap_or(0);
            st.set_page(page);
            {
                let mut v = params.values.borrow_mut();
                v.set(ParamId::Mode, f64::from(mode % 2));
                if let Some(id) = page.switch() {
                    v.set(id, 1.0);
                }
            }
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
                vocoder_notes: 0,
                midi_out_notes: 0,
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
