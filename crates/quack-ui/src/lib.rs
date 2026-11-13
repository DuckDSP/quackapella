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

mod assist;
mod brand;
mod controls;
mod eq;
pub mod library;
pub mod link;
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
    /// Host tempo, BPM (0 while unknown).
    pub tempo: f32,
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
    /// Gain reduction of the input compressor and de-esser, dB (≤ 0).
    pub comp_db: f32,
    pub deess_db: f32,
    /// The same, for the output dynamics.
    pub out_comp_db: f32,
    pub out_deess_db: f32,
    /// Notes the MIDI output plays, bit n = note n.
    pub midi_out_notes: u128,
    /// A Quackapella Link sends the mix, the carrier (in that order).
    pub link: [bool; 2],
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
    /// Layer whose effects the mixer shows (the last layer page).
    mix_layer: Page,
    /// Preset browser, values of this frame.
    presets: perform::PresetUi,
    /// Effect shown on the FX page.
    fx_tab: FxTab,
    /// The output EQ's graph: selected band, drag, spectrum.
    eq: eq::EqUi,
    /// The mixing assistant: its listen, diagnosis and undo.
    assist: assist::AssistUi,
    /// Vocoder page's tab: 0 voice and effects, 1 carrier.
    vocoder_tab: usize,
    /// Perform row lists open, and where they hang from.
    keys_menu: bool,
    voice_menu: bool,
    channel_menu: bool,
    source_menu: bool,
    keys_rect: egui::Rect,
    voice_rect: egui::Rect,
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
            mix_layer: Page::Voice,
            presets: perform::PresetUi::default(),
            fx_tab: FxTab::default(),
            eq: eq::EqUi::default(),
            assist: assist::AssistUi::default(),
            vocoder_tab: 0,
            keys_menu: false,
            voice_menu: false,
            channel_menu: false,
            source_menu: false,
            keys_rect: egui::Rect::NOTHING,
            voice_rect: egui::Rect::NOTHING,
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
    /// The voice effects: not a layer, the bus after them.
    Fx,
}

impl Page {
    /// The layers.
    pub const ALL: [Self; 4] = [Self::Voice, Self::Creative, Self::Harmony, Self::Vocoder];
    /// Every page: the layers, then the effects.
    pub const PAGES: [Self; 5] = [
        Self::Voice,
        Self::Creative,
        Self::Harmony,
        Self::Vocoder,
        Self::Fx,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Voice => "Voice",
            Self::Creative => "Creative",
            Self::Harmony => "Harmony",
            Self::Vocoder => "Vocoder",
            Self::Fx => "FX",
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
            Self::Fx => Some(ParamId::FxOn),
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
            Self::Fx => ParamId::FxLevel,
        }
    }
}

impl UiState {
    /// Show a layer's page in the big panel.
    pub fn set_page(&mut self, page: Page) {
        self.page = page;
        if page != Page::Fx {
            self.mix_layer = page;
        }
    }

    /// Close an edit left open (window closed mid-drag).
    pub fn end_open_edits(&mut self, params: &dyn ParamAccess) {
        if let Some((id, _)) = self.drag.take() {
            params.end_edit(id);
        }
        eq::end_drag(params, self);
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
        Page::Fx => color::FX,
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
    // The FX page gives its eight tabs a little more room.
    let tune_w = if st.page == Page::Fx { 420.0 } else { 452.0 };
    let tune_r = Rect::from_min_size(pos2(inner.left(), mid_top), vec2(tune_w, mid_h));
    assist::tick(params, st, view);
    if st.page == Page::Fx && !st.assist.open {
        // The FX page takes the whole row: the output EQ's graph where
        // the tune panel is on the layers' pages.
        eq::graph_panel(ui, params, st, view, tune_r, color::FX);
    } else if st.page == Page::Fx {
        assist::assistant_panel(ui, params, st, view, tune_r, color::FX);
    } else {
        tune_panel(ui, params, st, view, tune_r, accent, mode);
    }
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
        Page::Fx => fx_panel(ui, params, st, view, graph_r, page_acc),
    }

    // Bottom row: voice, level, MIDI.
    let row_top = tune_r.bottom() + space::M;
    let foot_h = 26.0;
    let macro_h = 92.0;
    let perform_h = 72.0;
    let row_h = inner.bottom() - foot_h - macro_h - perform_h - 2.0 * space::M - row_top;
    let voice_r = Rect::from_min_size(pos2(inner.left(), row_top), vec2(452.0, row_h));
    voice_panel(ui, params, st, voice_r, accent);
    // The mixer, where the level and MIDI-in panels were: the levels,
    // then each layer's delay, reverb, chorus and drive.
    let split_x = voice_r.right() + space::M + 250.0 + space::M;
    let mix_r = Rect::from_min_max(
        pos2(voice_r.right() + space::M, row_top),
        pos2(inner.right(), row_top + row_h),
    );
    mixer_panel(ui, params, st, mix_r);
    let midi_r = Rect::from_min_max(pos2(split_x, row_top), mix_r.max);

    // Perform row: snapshots and morph under the voice and mixer,
    // MIDI out under MIDI in.
    let perform_top = midi_r.bottom() + space::M;
    let snap_r = Rect::from_min_max(
        pos2(inner.left(), perform_top),
        pos2(split_x - space::M, perform_top + perform_h),
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
    if (st.drag.is_some() || st.eq.dragging()) && !ui.input(|i| i.pointer.any_down()) {
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
    let right_w = 262.0;
    let voice_r = Rect::from_min_size(pos2(r.right() - space::L - 150.0, row1), vec2(150.0, 28.0));
    let range = VoiceRange::from_index(params.get_plain(ParamId::Voice));
    if controls::stepper(
        ui,
        params,
        ParamId::Voice,
        voice_r,
        range.name(),
        color::QUACK,
    ) {
        st.voice_menu = !st.voice_menu;
    }
    st.voice_rect = voice_r;
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
    let gap = 12.0;
    // The layers, then the effects' cell after a gap (they come last,
    // whatever the routing).
    let cells = order.len() + 1;
    #[allow(clippy::cast_precision_loss)]
    let w = (right - left - gap * (cells - 1) as f32) / cells as f32;
    for (i, page) in order.into_iter().chain([Page::Fx]).enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let x = left + (w + gap) * i as f32;
        let acc = page_accent(page, style);
        let on = layer_on(params, page);
        let tab = Rect::from_min_size(pos2(x, row1), vec2(w, 28.0));
        layer_tab(ui, params, st, tab, page, on, acc);
        let fader = Rect::from_min_size(pos2(x, row2), vec2(w, 28.0));
        let label = match page {
            Page::Voice => "Lead",
            _ => "Level",
        };
        controls::fader(
            ui,
            params,
            st,
            page.level(),
            fader,
            if on { acc } else { acc.gamma_multiply(0.35) },
            label,
        );
        if series && i > 0 && page != Page::Fx {
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
        if page != Page::Fx {
            st.mix_layer = page;
        }
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
    let throat = params.get_plain(ParamId::Throat);
    let (line1, line2) = voice_summary(shift, formant);
    let line2 = throat_summary(throat, params.get_plain(ParamId::ThroatPitch)).unwrap_or(line2);
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
    // Keep In Key: the transposition before the correction, so the
    // voice lands on the scale.
    let kk = Rect::from_min_size(pos2(r.left() + 76.0, r.top() + 8.0), vec2(112.0, 24.0));
    controls::toggle(ui, params, ParamId::KeepInKey, kk, "Keep in Key", accent);
    let st_fmt = |v: f64| format!("{v:+.1}");
    let ct_fmt = |v: f64| format!("{v:+.0}");
    let pct = |v: f64| format!("{v:.0}");
    let kw = r.width() / 5.0;
    let ky = r.center().y - 4.0;
    let knobs: [(ParamId, &dyn Fn(f64) -> String); 5] = [
        (ParamId::Pitch, &st_fmt),
        (ParamId::Fine, &ct_fmt),
        (ParamId::Formant, &st_fmt),
        (ParamId::Throat, &ct_fmt),
        (ParamId::ThroatPitch, &pct),
    ];
    for (i, (id, fmt)) in knobs.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = r.left() + kw * (i as f32 + 0.5);
        controls::knob_sized(ui, params, st, id, pos2(cx, ky), 60.0, accent, fmt);
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
    let row1 = r.top() + 108.0;
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
        // And its place in the stereo field.
        let pr = fr.translate(vec2(0.0, 26.0));
        controls::fader(
            ui,
            params,
            st,
            quack_dsp::params::HARMONY_PAN_PARAMS[i],
            pr,
            if live { accent } else { dim },
            "",
        );
        if i == quack_dsp::harmony::INTERVAL_VOICES - 1 {
            // Beside the last voice's boxes, never over them.
            let x = fr.right() + space::M;
            paint::text_fit(
                p,
                pos2(x, pr.center().y),
                Align2::LEFT_CENTER,
                "← Level and pan",
                size::CAPTION - 1.0,
                color::TEXT_FAINT,
                r.right() - space::L - x,
            );
        }
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
    let knobs: [Knob; 8] = [
        (ParamId::Spread, "Spread", &pct),
        (ParamId::HarmonyHumanize, "Humanize", &off),
        (ParamId::HarmonyVibrato, "Vibrato", &off),
        (ParamId::HarmonyTiming, "Timing", &off),
        (ParamId::Character, "Timbre", &off),
        (ParamId::Expression, "Express", &pct),
        (ParamId::Glide, "Glide", &ms),
        (ParamId::Double, "Double", &off),
    ];
    let kw8 = kw * 6.0 / 8.0;
    for (i, (id, label, fmt)) in knobs.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = r.left() + space::L + kw8 * (i as f32 + 0.5);
        controls::knob_named(
            ui,
            params,
            st,
            id,
            pos2(cx, row2),
            knob_d.min(kw8 - space::S),
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
    let dim = accent.gamma_multiply(0.35);
    let swapped = params.get_plain(ParamId::VocoderSwap) >= 0.5;
    controls::segmented(
        ui,
        params,
        ParamId::Carrier,
        car_r,
        &cnames,
        &[if swapped { dim } else { accent }],
    );
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
    let carrier = if swapped { Carrier::Sidechain } else { carrier };
    let status = match (mode, carrier) {
        _ if !layer => "Off".to_owned(),
        _ if swapped && view.link[1] => "Voice: Link · Carrier: track".to_owned(),
        _ if swapped => "Voice: side-chain · Carrier: track".to_owned(),
        (_, Carrier::Sidechain) if view.link[1] => "Quackapella Link".to_owned(),
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

    let knob_d = 50.0;
    let off = |v: f64| {
        if v < 0.5 {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let ms = |v: f64| format!("{v:.0}");
    let pct = |v: f64| format!("{v:.0}");
    let st_fmt = |v: f64| format!("{v:+.1}");
    let db_fmt = |v: f64| format!("{v:+.0}");
    let gate_fmt = |v: f64| {
        if v <= quack_dsp::vocoder::GATE_OFF_DB + 0.5 {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let bands = |v: f64| format!("{}", bands_from_index(v));
    #[allow(clippy::cast_possible_truncation)]
    let octave = |v: f64| format!("{:+}", v.round() as i32 - 2);
    let vocoder_only = mode != VocoderMode::Talkbox;
    let pitched = !matches!(carrier, Carrier::Noise | Carrier::Sidechain);
    let synth = carrier != Carrier::Sidechain;
    // Tabs: the voice and the effects, or the carrier synth; the inputs
    // swap on the right (the voice by the side-chain, like the EVOC).
    let tab_y = car_r.bottom() + space::S + 26.0 + space::S;
    let tr = Rect::from_min_size(pos2(car_r.left(), tab_y), vec2(220.0, 24.0));
    controls::tabs(
        ui,
        "vocoder",
        tr,
        &["Voice & FX", "Carrier"],
        &mut st.vocoder_tab,
        accent,
    );
    let sr = Rect::from_min_size(pos2(car_r.right() - 190.0, tab_y), vec2(190.0, 24.0));
    controls::toggle(
        ui,
        params,
        ParamId::VocoderSwap,
        sr,
        "Voice from side-chain",
        accent,
    );
    type Knob<'a> = (ParamId, &'a str, &'a dyn Fn(f64) -> String, bool);
    let voice_row1: [Knob; 6] = [
        (ParamId::Bands, "Bands", &bands, vocoder_only),
        (ParamId::Response, "Response", &ms, vocoder_only),
        (ParamId::BandShift, "Shift", &st_fmt, true),
        (ParamId::VocoderResonance, "Resonance", &pct, true),
        (ParamId::VocoderTilt, "Tilt", &db_fmt, true),
        (ParamId::VocoderGate, "Gate", &gate_fmt, true),
    ];
    let voice_row2: [Knob; 5] = [
        (ParamId::Sibilance, "Sibilance", &off, true),
        (ParamId::Consonants, "Consonants", &off, true),
        (ParamId::VocoderWidth, "Width", &off, true),
        (ParamId::Ensemble, "Ensemble", &off, true),
        (ParamId::VocoderLofi, "Lo-Fi", &off, true),
    ];
    let carrier_row1: [Knob; 6] = [
        (ParamId::CarrierOctave, "Octave", &octave, pitched),
        (ParamId::Detune, "Detune", &off, pitched),
        (ParamId::CarrierSub, "Sub", &off, pitched),
        (ParamId::CarrierNoise, "Noise", &off, pitched),
        (ParamId::CarrierGlide, "Glide", &off, pitched),
        (ParamId::CarrierRelease, "Release", &ms, synth),
    ];
    let carrier_row2: [Knob; 1] = [(ParamId::BendRange, "Bend", &st_fmt, pitched)];
    let kw = (r.width() - 2.0 * space::L) / 6.0;
    let y1 = tab_y + 24.0 + 12.0 + knob_d / 2.0;
    let y2 = r.bottom() - 58.0;
    let rows: [(&[Knob], f32); 2] = if st.vocoder_tab == 0 {
        [(&voice_row1[..], y1), (&voice_row2[..], y2)]
    } else {
        [(&carrier_row1[..], y1), (&carrier_row2[..], y2)]
    };
    for (row, y) in &rows {
        for (i, (id, label, fmt, live)) in row.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let cx = r.left() + space::L + kw * (i as f32 + 0.5 + (6 - row.len()) as f32 / 2.0);
            controls::knob_named(
                ui,
                params,
                st,
                *id,
                pos2(cx, *y),
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

/// The mixer, two blocks side by side: the effects of the layer shown
/// (its delay and reverb sends, its chorus and drive amounts, in its
/// colour), then the global levels (in blue).
fn mixer_panel(ui: &egui::Ui, params: &dyn ParamAccess, st: &mut UiState, r: Rect) {
    let p = ui.painter();
    let mode = VoiceMode::from_index(params.get_plain(ParamId::Mode));
    let series = Routing::from_index(params.get_plain(ParamId::Routing)) == Routing::Series;
    // The FX page keeps the last layer shown; in series one set of
    // amounts serves the whole chain (the voice's).
    let layer = if series { Page::Voice } else { st.mix_layer };
    let col = Page::ALL.iter().position(|&l| l == layer).unwrap_or(0);
    let fx_w = (r.width() - space::M) * 4.0 / 7.0;
    let fx_r = Rect::from_min_size(r.min, vec2(fx_w, r.height()));
    let gl_r = Rect::from_min_max(pos2(fx_r.right() + space::M, r.top()), r.max);
    panel(p, fx_r);
    panel(p, gl_r);

    let acc = page_accent(layer, mode);
    let layer_live = layer_on(params, layer);
    let title = if series {
        "FX · Chain".to_owned()
    } else {
        format!("FX · {}", layer.name())
    };
    let title_y = r.top() + 20.0;
    text(
        p,
        pos2(fx_r.left() + space::L, title_y),
        Align2::LEFT_CENTER,
        &title,
        size::LABEL,
        if layer_live { acc } else { color::TEXT_FAINT },
    );
    text(
        p,
        pos2(fx_r.right() - space::L, title_y),
        Align2::RIGHT_CENTER,
        if series {
            "Whole chain"
        } else {
            "Pick a layer above"
        },
        size::CAPTION,
        color::TEXT_FAINT,
    );
    let fx = params.get_plain(ParamId::FxOn) >= 0.5;
    let fx_switch = |id: ParamId| fx && params.get_plain(id) >= 0.5;
    let off = |v: f64| {
        if v < 0.5 {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let knob_y = r.top() + 40.0 + (r.height() - 40.0) / 2.0 - 10.0;
    let knobs: [(&str, [ParamId; 4], bool); 4] = [
        (
            "Delay",
            quack_dsp::params::DELAY_SEND_PARAMS,
            fx_switch(ParamId::DelayOn),
        ),
        (
            "Reverb",
            quack_dsp::params::REVERB_SEND_PARAMS,
            fx_switch(ParamId::ReverbOn),
        ),
        (
            "Chorus",
            quack_dsp::params::CHORUS_AMOUNT_PARAMS,
            fx_switch(ParamId::ChorusOn),
        ),
        (
            "Drive",
            quack_dsp::params::DRIVE_AMOUNT_PARAMS,
            fx_switch(ParamId::DriveOn),
        ),
    ];
    #[allow(clippy::cast_precision_loss)]
    let kw = (fx_r.width() - 2.0 * space::L) / knobs.len() as f32;
    for (i, (name, ids, on)) in knobs.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = fx_r.left() + space::L + kw * (i as f32 + 0.5);
        let a = if layer_live && *on {
            acc
        } else {
            acc.gamma_multiply(0.35)
        };
        controls::knob_named(
            ui,
            params,
            st,
            ids[col],
            pos2(cx, knob_y),
            54.0,
            0.0,
            a,
            &off,
            name,
        );
    }

    text(
        p,
        pos2(gl_r.left() + space::L, title_y),
        Align2::LEFT_CENTER,
        "Global",
        size::LABEL,
        color::TUNE,
    );
    let gain = |v: f64| format!("{v:+.0}");
    let pct = |v: f64| format!("{v:.0}");
    #[allow(clippy::type_complexity)]
    let levels: [(ParamId, &str, f64, &dyn Fn(f64) -> String); 3] = [
        (ParamId::InputGain, "In", 0.0, &gain),
        (ParamId::Mix, "Mix", 0.0, &pct),
        (ParamId::OutputGain, "Out", 0.0, &gain),
    ];
    let gw = (gl_r.width() - 2.0 * space::L) / 3.0;
    for (i, (id, name, floor, fmt)) in levels.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = gl_r.left() + space::L + gw * (i as f32 + 0.5);
        controls::knob_named(
            ui,
            params,
            st,
            id,
            pos2(cx, knob_y),
            54.0,
            floor,
            color::TUNE,
            fmt,
            name,
        );
    }
}

/// The effects of the FX page, one tab each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FxTab {
    Tone,
    #[default]
    Input,
    Drive,
    Chorus,
    /// Output EQ and exciter (after the chorus, before the returns).
    Eq,
    Delay,
    Reverb,
    Output,
}

impl FxTab {
    pub const ALL: [Self; 8] = [
        Self::Tone,
        Self::Input,
        Self::Drive,
        Self::Chorus,
        Self::Eq,
        Self::Delay,
        Self::Reverb,
        Self::Output,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tone => "Tone",
            Self::Input => "Input",
            Self::Drive => "Drive",
            Self::Chorus => "Chorus",
            Self::Eq => "EQ",
            Self::Delay => "Delay",
            Self::Reverb => "Reverb",
            Self::Output => "Output",
        }
    }

    /// The effect's own switch (the sends have none).
    #[must_use]
    pub const fn switch(self) -> Option<ParamId> {
        match self {
            Self::Tone => Some(ParamId::ToneOn),
            Self::Input => Some(ParamId::DynamicsOn),
            Self::Drive => Some(ParamId::DriveOn),
            Self::Chorus => Some(ParamId::ChorusOn),
            Self::Eq => Some(ParamId::EqOn),
            Self::Delay => Some(ParamId::DelayOn),
            Self::Reverb => Some(ParamId::ReverbOn),
            Self::Output => Some(ParamId::OutputOn),
        }
    }

    /// What the effect does, under the tabs.
    const fn caption(self) -> &'static str {
        match self {
            Self::Tone => "Input EQ, before every layer",
            Self::Input => "Gate, de-esser and compressor on the input",
            Self::Drive => "Each layer's amount is in the mixer below",
            Self::Chorus => "Each layer's amount is in the mixer below",
            Self::Eq => "EQ and exciter on the transformed sound, after the layers",
            Self::Delay => "Each layer's send is in the mixer below",
            Self::Reverb => "Each layer's send is in the mixer below",
            Self::Output => "De-esser, compressor and limiter on the whole sound",
        }
    }
}

impl UiState {
    /// Show an effect on the FX page.
    /// Open the preset browser on `shelf` (the loaded preset's when
    /// `None`), as a click on the preset name does.
    pub fn open_presets(&mut self, shelf: Option<perform::Shelf>) {
        self.presets.open_browser(shelf);
    }

    pub fn set_fx_tab(&mut self, tab: FxTab) {
        self.fx_tab = tab;
    }

    /// The engine's analysis taps, for the EQ graph's spectrum.
    pub fn set_taps(&mut self, taps: quack_dsp::tap::Taps) {
        self.eq.taps = Some(taps);
    }

    /// Open the mixing assistant on a diagnosis (captures and tests).
    pub fn show_assistant(
        &mut self,
        analysis: quack_dsp::assist::Analysis,
        style: quack_dsp::assist::Style,
        tempo: u32,
    ) {
        self.assist.show(analysis, style, tempo);
    }
}

type FxKnob<'a> = (ParamId, &'a str, &'a dyn Fn(f64) -> String, Color32);

/// One row of knobs across `r`, in `cols` slots from slot `first`.
#[allow(clippy::too_many_arguments)]
fn fx_knobs(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    r: Rect,
    y: f32,
    cols: usize,
    first: usize,
    knobs: &[FxKnob],
) {
    #[allow(clippy::cast_precision_loss)]
    let kw = (r.width() - 2.0 * space::L) / cols as f32;
    for (i, (id, name, fmt, acc)) in knobs.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let cx = r.left() + space::L + kw * ((first + i) as f32 + 0.5);
        controls::knob_named(
            ui,
            params,
            st,
            *id,
            pos2(cx, y),
            54.0,
            id.spec().kind.min(),
            *acc,
            *fmt,
            name,
        );
    }
}

/// The tabs of the FX page: a switch (dot) and the name, which shows
/// the effect.
fn fx_tabs(ui: &egui::Ui, params: &dyn ParamAccess, st: &mut UiState, r: Rect, accent: Color32) {
    let p = ui.painter();
    let gap = space::XS;
    #[allow(clippy::cast_precision_loss)]
    let n = FxTab::ALL.len() as f32;
    let w = (r.width() - gap * (n - 1.0)) / n;
    for (i, tab) in FxTab::ALL.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let x = r.left() + (w + gap) * i as f32;
        let cell = Rect::from_min_size(pos2(x, r.top()), vec2(w, r.height()));
        let shown = st.fx_tab == tab;
        let on = tab.switch().is_none_or(|id| params.get_plain(id) >= 0.5);
        let dot_w = if tab.switch().is_some() { 17.0 } else { 0.0 };
        let name_r = Rect::from_min_max(pos2(cell.left() + dot_w, cell.top()), cell.max);
        let resp = ui.interact(name_r, ui.id().with(("quack-fx-tab", i)), Sense::click());
        if resp.clicked() {
            st.fx_tab = tab;
        }
        let (fill, stroke) = if shown {
            (accent.gamma_multiply(0.22), accent.gamma_multiply(0.8))
        } else if resp.hovered() {
            (color::BORDER, color::BORDER)
        } else {
            (color::RAISED, color::BORDER)
        };
        paint::rounded(p, cell, radius::CONTROL, fill, stroke);
        text(
            p,
            if tab.switch().is_some() {
                name_r.left_center()
            } else {
                name_r.center()
            },
            if tab.switch().is_some() {
                Align2::LEFT_CENTER
            } else {
                Align2::CENTER_CENTER
            },
            tab.name(),
            size::CAPTION - 1.0,
            if shown {
                accent
            } else if on {
                color::TEXT_DIM
            } else {
                color::TEXT_FAINT
            },
        );
        if let Some(id) = tab.switch() {
            let dot_r = Rect::from_min_size(cell.min, vec2(dot_w, cell.height()));
            let presp = ui.interact(dot_r, ui.id().with(("quack-fx-on", i)), Sense::click());
            if presp.clicked() {
                set_once(params, id, if on { 0.0 } else { 1.0 });
            }
            crate::perform::attach(ui, &presp, params, id);
            let dot = dot_r.center() + vec2(0.5, 0.0);
            let ring = if presp.hovered() {
                color::TEXT_DIM
            } else {
                color::TEXT_FAINT
            };
            p.circle_stroke(
                dot,
                4.5,
                egui::Stroke::new(1.5_f32, if on { accent } else { ring }),
            );
            if on {
                p.circle_filled(dot, 2.5, accent);
            }
        }
    }
}

/// The voice effects, one tab each: tone and dynamics at the input,
/// delay and reverb on a bus fed by each layer's send, the chorus after
/// the layers.
fn fx_panel(
    ui: &egui::Ui,
    params: &dyn ParamAccess,
    st: &mut UiState,
    view: &View,
    r: Rect,
    accent: Color32,
) {
    let p = ui.painter();
    panel(p, r);
    layer_switch(ui, params, ParamId::FxOn, r, "FX", accent);
    let on = params.get_plain(ParamId::FxOn) >= 0.5;
    let tab = st.fx_tab;
    let tab_on = on && tab.switch().is_none_or(|id| params.get_plain(id) >= 0.5);
    let dim = accent.gamma_multiply(0.35);
    let live = |b: bool| if b && tab_on { accent } else { dim };
    // Gain reduction while it works, else what the page does.
    let dyn_on = on && params.get_plain(ParamId::DynamicsOn) >= 0.5;
    let reducing = dyn_on && (view.comp_db < -0.1 || view.deess_db < -0.1);
    let status = if !on {
        "Off · no latency added".to_owned()
    } else if reducing {
        format!(
            "Comp {:.1} dB · De-ess {:.1} dB",
            view.comp_db, view.deess_db
        )
    } else {
        "No latency added".to_owned()
    };
    text(
        p,
        pos2(r.right() - space::L, r.top() + 22.0),
        Align2::RIGHT_CENTER,
        &status,
        size::CAPTION,
        if reducing { accent } else { color::TEXT_FAINT },
    );
    let tabs_r = Rect::from_min_size(
        pos2(r.left() + space::L, r.top() + 46.0),
        vec2(r.width() - 2.0 * space::L, 28.0),
    );
    fx_tabs(ui, params, st, tabs_r, accent);
    text(
        p,
        pos2(r.left() + space::L, tabs_r.bottom() + 16.0),
        Align2::LEFT_CENTER,
        tab.caption(),
        size::CAPTION,
        color::TEXT_FAINT,
    );

    let off = |v: f64| {
        if v < 0.5 {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let pct = |v: f64| format!("{v:.0}");
    let db = |v: f64| format!("{v:.0}");
    let sdb = |v: f64| format!("{v:+.1}");
    let hz = |v: f64| {
        if v >= 1000.0 {
            format!("{:.1}k", v / 1000.0)
        } else {
            format!("{v:.0}")
        }
    };
    let low_cut = |v: f64| {
        if v <= f64::from(quack_dsp::fx::LOW_CUT_OFF_HZ) + 0.01 {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let gate = |v: f64| {
        if v <= f64::from(quack_dsp::fx::GATE_OFF_DB) + 0.01 {
            "Off".to_owned()
        } else {
            format!("{v:.0}")
        }
    };
    let ratio = |v: f64| {
        if v < 1.01 {
            "Off".to_owned()
        } else {
            format!("{v:.1}")
        }
    };
    let ms = |v: f64| {
        if v < 10.0 {
            format!("{v:.1}")
        } else {
            format!("{v:.0}")
        }
    };
    let secs = |v: f64| format!("{v:.1}s");
    let rate = |v: f64| format!("{v:.2}");
    let y1 = r.top() + 152.0;
    let y2 = r.bottom() - 58.0;
    let dyn_bars = |p: &egui::Painter, at: Rect, on: bool, rows: [(&str, f32); 2]| {
        for (i, (name, v)) in rows.into_iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let y = at.top() + 13.0 + 26.0 * i as f32;
            text(
                p,
                pos2(at.left(), y),
                Align2::LEFT_CENTER,
                name,
                size::CAPTION,
                color::TEXT_FAINT,
            );
            let bar =
                Rect::from_min_max(pos2(at.left() + 56.0, y - 5.0), pos2(at.right(), y + 5.0));
            paint::rounded(p, bar, 2, color::WELL, color::BORDER);
            let frac = if on { (-v / 18.0).clamp(0.0, 1.0) } else { 0.0 };
            if frac > 0.0 {
                let fill =
                    Rect::from_min_max(pos2(bar.right() - bar.width() * frac, bar.top()), bar.max);
                paint::rounded(p, fill, 2, accent, accent);
            }
        }
    };
    let inner_w = r.width() - 2.0 * space::L;
    match tab {
        FxTab::Tone => {
            let k: [FxKnob; 3] = [
                (ParamId::LowCut, "Low Cut", &low_cut, live(true)),
                (ParamId::Presence, "Presence", &sdb, live(true)),
                (ParamId::Air, "Air", &sdb, live(true)),
            ];
            fx_knobs(ui, params, st, r, (y1 + y2) / 2.0, 3, 0, &k);
        }
        FxTab::Input => {
            let comp = params.get_plain(ParamId::CompRatio) > 1.01;
            let deess = params.get_plain(ParamId::DeEss) > 0.5;
            let k1: [FxKnob; 3] = [
                (ParamId::Gate, "Gate", &gate, live(true)),
                (ParamId::DeEss, "De-Ess", &off, live(true)),
                (ParamId::DeEssFreq, "De-Ess Freq", &hz, live(deess)),
            ];
            fx_knobs(ui, params, st, r, y1, 5, 0, &k1);
            let meter = Rect::from_min_size(
                pos2(r.left() + space::L + inner_w * 0.62, y1 - 26.0),
                vec2(inner_w * 0.38, 52.0),
            );
            dyn_bars(
                p,
                meter,
                dyn_on,
                [("Comp", view.comp_db), ("De-ess", view.deess_db)],
            );
            let k2: [FxKnob; 5] = [
                (ParamId::CompThreshold, "Threshold", &db, live(comp)),
                (ParamId::CompRatio, "Ratio", &ratio, live(true)),
                (ParamId::CompAttack, "Attack", &ms, live(comp)),
                (ParamId::CompRelease, "Release", &ms, live(comp)),
                (ParamId::CompMakeup, "Makeup", &db, live(true)),
            ];
            fx_knobs(ui, params, st, r, y2, 5, 0, &k2);
        }
        FxTab::Drive => {
            let names: Vec<&str> = quack_dsp::fx::DriveStyle::ALL
                .iter()
                .map(|d| d.name())
                .collect();
            let seg =
                Rect::from_min_size(pos2(r.left() + space::L, y1 - 14.0), vec2(inner_w, 28.0));
            controls::segmented(ui, params, ParamId::DriveStyle, seg, &names, &[live(true)]);
            let k: [FxKnob; 2] = [
                (ParamId::Drive, "Drive", &pct, live(true)),
                (ParamId::DriveTone, "Tone", &pct, live(true)),
            ];
            fx_knobs(ui, params, st, r, y2, 4, 1, &k);
        }
        FxTab::Chorus => {
            let k: [FxKnob; 2] = [
                (ParamId::ChorusRate, "Rate", &rate, live(true)),
                (ParamId::ChorusDepth, "Depth", &pct, live(true)),
            ];
            fx_knobs(ui, params, st, r, (y1 + y2) / 2.0, 4, 1, &k);
        }
        FxTab::Eq => eq::tab(ui, params, st, r, y1, y2, live(true), dim),
        FxTab::Delay => {
            let sync = params.get_plain(ParamId::DelaySync) >= 0.5;
            let ping = params.get_plain(ParamId::DelayPingPong) >= 0.5;
            let k1: [FxKnob; 5] = [
                (ParamId::DelayLevel, "Level", &off, live(true)),
                (ParamId::DelayTime, "Time", &ms, live(!sync)),
                (ParamId::DelayFeedback, "Feedback", &pct, live(true)),
                (ParamId::DelayTone, "Tone", &pct, live(true)),
                (ParamId::DelayLowCut, "Low Cut", &hz, live(true)),
            ];
            fx_knobs(ui, params, st, r, y1, 5, 0, &k1);
            // Sync, its note value and ping-pong in the first column,
            // then the options.
            let kw = inner_w / 5.0;
            let ty = y2 - 43.0;
            let cell = |i: f32| {
                Rect::from_min_size(
                    pos2(r.left() + space::L, ty + 30.0 * i),
                    vec2(kw - space::XS, 26.0),
                )
            };
            controls::toggle(
                ui,
                params,
                ParamId::DelaySync,
                cell(0.0),
                "Sync",
                live(true),
            );
            let div = quack_dsp::fx::Division::from_index(params.get_plain(ParamId::DelayDivision));
            controls::stepper(
                ui,
                params,
                ParamId::DelayDivision,
                cell(1.0),
                div.name(),
                live(sync),
            );
            controls::toggle(
                ui,
                params,
                ParamId::DelayPingPong,
                cell(2.0),
                "Ping-Pong",
                live(true),
            );
            let k2: [FxKnob; 4] = [
                (ParamId::DelayMod, "Wobble", &off, live(true)),
                (ParamId::DelayWidth, "Width", &off, live(!ping)),
                (ParamId::DelayToReverb, "To Reverb", &off, live(true)),
                (ParamId::DelayDuck, "Duck", &off, live(true)),
            ];
            fx_knobs(ui, params, st, r, y2, 5, 1, &k2);
        }
        FxTab::Reverb => {
            let k1: [FxKnob; 5] = [
                (ParamId::ReverbLevel, "Level", &off, live(true)),
                (ParamId::ReverbDecay, "Decay", &secs, live(true)),
                (ParamId::ReverbSize, "Size", &pct, live(true)),
                (ParamId::ReverbPredelay, "Pre-Delay", &ms, live(true)),
                (ParamId::Duck, "Duck", &off, live(true)),
            ];
            fx_knobs(ui, params, st, r, y1, 5, 0, &k1);
            let k2: [FxKnob; 4] = [
                (ParamId::ReverbDamping, "Damping", &pct, live(true)),
                (ParamId::ReverbLowCut, "Low Cut", &hz, live(true)),
                (ParamId::ReverbMod, "Modulation", &off, live(true)),
                (ParamId::ReverbWidth, "Width", &pct, live(true)),
            ];
            fx_knobs(ui, params, st, r, y2, 5, 0, &k2);
        }
        FxTab::Output => {
            let out_on = on && params.get_plain(ParamId::OutputOn) >= 0.5;
            let ceiling = |v: f64| format!("{v:.1}");
            let k: [FxKnob; 3] = [
                (ParamId::OutDeEss, "De-Ess", &off, live(true)),
                (ParamId::OutComp, "Comp", &off, live(true)),
                (ParamId::OutCeiling, "Ceiling", &ceiling, live(true)),
            ];
            fx_knobs(ui, params, st, r, y1, 3, 0, &k);
            let meter = Rect::from_min_size(
                pos2(r.left() + space::L + inner_w * 0.2, y2 - 26.0),
                vec2(inner_w * 0.6, 52.0),
            );
            dyn_bars(
                p,
                meter,
                out_on,
                [("Comp", view.out_comp_db), ("De-ess", view.out_deess_db)],
            );
        }
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

/// What the throat does, for the voice panel (`None` at rest).
#[must_use]
pub fn throat_summary(throat_pct: f64, link_pct: f64) -> Option<String> {
    if throat_pct.abs() < 0.5 {
        return None;
    }
    let t = throat_pct / 100.0;
    let who = match t {
        t if t <= -0.75 => "child",
        t if t <= -0.35 => "younger, smaller",
        t if t < 0.0 => "a little smaller",
        t if t < 0.35 => "a little bigger",
        t if t < 0.75 => "bigger, deeper",
        _ => "giant",
    };
    let pitch = quack_dsp::throat::pitch_st(t, link_pct / 100.0);
    let fmt = quack_dsp::throat::formant_st(t);
    Some(if pitch.abs() < 0.05 {
        format!("Throat: {who} · formants {fmt:+.1} st, body and air")
    } else {
        format!("Throat: {who} · formants {fmt:+.1} st, pitch {pitch:+.1} st")
    })
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
        for (mode, page) in Page::PAGES.into_iter().enumerate() {
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
                tempo: 120.0,
                pitch_hz: if mode % 2 == 0 { 0.0 } else { 223.1 },
                pitch_confidence: 0.92,
                tune_sung: 57.2,
                tune_target: 57.0,
                tune_out: 57.02,
                tune_mask: 0b1010_1011_0101,
                frozen: mode == 2,
                harmony_notes: if mode == 3 { 1 << 64 } else { 0 },
                vocoder_notes: 0,
                comp_db: -3.5,
                deess_db: -1.0,
                out_comp_db: 0.0,
                out_deess_db: 0.0,
                midi_out_notes: 0,
                link: [false; 2],
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
