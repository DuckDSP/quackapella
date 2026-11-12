//! The DuckDSP look, taken from Sonder (`mastering-ui/src/theme.rs`) so
//! the range reads as one family: same surfaces, text colours, type,
//! spacing and radii. Only the accents are Quackapella's.
//!
//! Every colour, size and radius the editor draws with comes from here. Labels are
//! set in Inter (OFL, `assets/Inter-OFL.txt`), values in JetBrains Mono,
//! which the plugin already ships through truce and passes to [`apply`].

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
    Visuals,
};

/// Inter Medium 4.1 (SIL Open Font License 1.1): titles.
pub const INTER: &[u8] = include_bytes!("../assets/Inter-Medium.ttf");
/// Inter Regular (SIL Open Font License 1.1): labels and body text.
pub const INTER_REGULAR: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");

/// Text at or above this size is set in Inter Medium (titles); smaller
/// text in Inter Regular.
pub const MEDIUM_FROM: f32 = 16.0;

fn medium_family() -> FontFamily {
    FontFamily::Name("inter-medium".into())
}

/// Colour tokens. Surfaces go from darkest (`BG`) to lightest
/// (`RAISED`); text from `TEXT` (values, titles) to `TEXT_FAINT`
/// (units, scales).
pub mod color {
    use egui::Color32;

    pub const BG: Color32 = Color32::from_rgb(0x12, 0x17, 0x1E);
    pub const PANEL: Color32 = Color32::from_rgb(0x1C, 0x25, 0x30);
    pub const RAISED: Color32 = Color32::from_rgb(0x23, 0x2D, 0x3A);
    /// Plots and meter wells: darker than the panel they sit on.
    pub const WELL: Color32 = Color32::from_rgb(0x12, 0x18, 0x20);
    pub const BORDER: Color32 = Color32::from_rgb(0x35, 0x41, 0x50);
    pub const GRID: Color32 = Color32::from_rgb(0x1E, 0x27, 0x33);
    pub const GRID_STRONG: Color32 = Color32::from_rgb(0x2D, 0x39, 0x48);

    pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xED, 0xF5);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(0xA3, 0xAF, 0xBF);
    pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x8A, 0x96, 0xA8);

    /// Quackapella's own accent (title, focus, selection).
    pub const QUACK: Color32 = Color32::from_rgb(0xF2, 0xC9, 0x4C);
    /// One accent per voice mode, from the most transparent to the most
    /// synthetic. Taken from Sonder's palette.
    pub const NATURAL: Color32 = Color32::from_rgb(0x6F, 0xCF, 0x9A);
    pub const TUNE: Color32 = Color32::from_rgb(0x6C, 0xB8, 0xFA);
    pub const CREATIVE: Color32 = Color32::from_rgb(0xC0, 0x9C, 0xF6);
    pub const SYNTH: Color32 = Color32::from_rgb(0xF2, 0x9A, 0x62);
    /// The vocoder layer, apart from the harmonies.
    pub const VOCODER: Color32 = Color32::from_rgb(0xEE, 0x7E, 0xA6);
    /// Voice effects (dynamics, delay, reverb).
    pub const FX: Color32 = Color32::from_rgb(0x5C, 0xD0, 0xC8);

    /// Meters: under −6 dBFS, up to the ceiling, over it.
    pub const METER_OK: Color32 = Color32::from_rgb(0x4F, 0xC2, 0xB4);
    pub const METER_HOT: Color32 = Color32::from_rgb(0xF2, 0xB8, 0x5A);
    pub const METER_OVER: Color32 = Color32::from_rgb(0xEC, 0x6E, 0x78);
}

/// Spacing scale, in points.
pub mod space {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
}

/// Corner radii, in points.
pub mod radius {
    pub const CONTROL: u8 = 5;
    pub const PANEL: u8 = 10;
}

/// Type sizes, in points.
pub mod size {
    /// Units, scales, captions. The smallest text: at the smallest
    /// zoom (75 %) it is still 10 logical pixels.
    pub const CAPTION: f32 = 13.5;
    /// Control labels.
    pub const LABEL: f32 = 13.5;
    pub const BODY: f32 = 14.0;
    /// Numeric read-outs under controls.
    pub const VALUE: f32 = 13.5;
    /// Module and section titles.
    pub const TITLE: f32 = 18.0;
    /// Large read-outs.
    pub const READOUT: f32 = 26.0;
}

#[must_use]
pub fn label_font(size: f32) -> FontId {
    if size >= MEDIUM_FROM {
        FontId::new(size, medium_family())
    } else {
        FontId::new(size, FontFamily::Proportional)
    }
}

#[must_use]
pub fn value_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Inter for proportional text, `mono` for monospace; egui's own fonts
/// stay behind them for the glyphs they lack.
#[must_use]
pub fn fonts(mono: &'static [u8]) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    defs.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(INTER_REGULAR)),
    );
    defs.font_data.insert(
        "inter-medium".into(),
        Arc::new(FontData::from_static(INTER)),
    );
    defs.font_data
        .insert("duckdsp-mono".into(), Arc::new(FontData::from_static(mono)));
    for (family, name) in [
        (FontFamily::Proportional, "inter"),
        (FontFamily::Monospace, "duckdsp-mono"),
    ] {
        defs.families
            .entry(family)
            .or_default()
            .insert(0, name.into());
    }
    let fallback = defs.families[&FontFamily::Proportional].clone();
    let mut medium = vec!["inter-medium".to_owned()];
    medium.extend(fallback.into_iter().filter(|n| n != "inter"));
    defs.families.insert(medium_family(), medium);
    defs
}

#[must_use]
pub fn text_styles() -> BTreeMap<TextStyle, FontId> {
    BTreeMap::from([
        (TextStyle::Small, label_font(size::CAPTION)),
        (TextStyle::Body, label_font(size::BODY)),
        (TextStyle::Button, label_font(size::BODY)),
        (TextStyle::Heading, label_font(size::TITLE)),
        (TextStyle::Monospace, value_font(size::VALUE)),
    ])
}

#[must_use]
pub fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.panel_fill = color::PANEL;
    v.window_fill = color::RAISED;
    v.window_stroke = Stroke::new(1.0_f32, color::BORDER);
    v.extreme_bg_color = color::WELL;
    v.faint_bg_color = color::RAISED;
    v.override_text_color = None;
    v.selection.bg_fill = color::QUACK.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, color::QUACK);
    let control = CornerRadius::same(radius::CONTROL);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = color::PANEL;
    w.noninteractive.weak_bg_fill = color::PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, color::BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, color::TEXT_DIM);
    for (state, bg, fg) in [
        (&mut w.inactive, color::RAISED, color::TEXT_DIM),
        (
            &mut w.hovered,
            Color32::from_rgb(0x2B, 0x37, 0x46),
            color::TEXT,
        ),
        (
            &mut w.active,
            Color32::from_rgb(0x33, 0x41, 0x52),
            color::TEXT,
        ),
        (&mut w.open, color::RAISED, color::TEXT),
    ] {
        state.bg_fill = bg;
        state.weak_bg_fill = bg;
        state.bg_stroke = Stroke::new(1.0_f32, color::BORDER);
        state.fg_stroke = Stroke::new(1.0_f32, fg);
        state.corner_radius = control;
    }
    w.noninteractive.corner_radius = control;
    v
}

/// Install fonts, text styles and visuals. `mono` is the value font
/// (the plugin passes JetBrains Mono).
pub fn apply(ctx: &egui::Context, mono: &'static [u8]) {
    ctx.set_fonts(fonts(mono));
    ctx.all_styles_mut(|style| {
        style.text_styles = text_styles();
        style.visuals = visuals();
        style.spacing.item_spacing = egui::vec2(space::S, space::XS);
        style.spacing.button_padding = egui::vec2(space::S, 3.0);
    });
}

/// WCAG 2 contrast ratio between two opaque colours (1 to 21).
#[must_use]
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    fn luminance(c: Color32) -> f32 {
        let lin = |u: u8| {
            let s = f32::from(u) / 255.0;
            if s <= 0.040_45 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}
