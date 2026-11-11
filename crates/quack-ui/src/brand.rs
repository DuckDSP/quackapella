//! The DuckDSP mark (footer), shared with Sonder: the approved vector
//! outlines (Sonder's `assets/branding`) as path data, rasterised once per
//! size with tiny-skia and cached as a texture on the UI thread. The
//! Quackapella wordmark has no approved artwork yet; the header sets it in
//! spaced Inter Medium until it does. Nothing here touches audio.

use egui::{Color32, Rect, TextureHandle, TextureOptions, Ui};
use std::sync::OnceLock;
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub(crate) enum Brand {
    DuckDsp,
}

/// One outline: SVG path data (absolute M L H V C S Z only), its offset,
/// and a stroke width (`None` fills it).
struct Glyph {
    d: &'static str,
    at: (f32, f32),
    stroke: Option<(f32, LineCap)>,
}

const B: LineCap = LineCap::Butt;
const Q: LineCap = LineCap::Square;

/// The DuckDSP mark shared with the rest of the range, viewBox 492 × 84.
const DUCKDSP: [Glyph; 8] = [
    Glyph {
        d: "M15 12H32C51 12 60 26 60 40S51 68 32 68H15V12Z M15 28H33V52H15 M58.709753 31L74 40L58.709753 49",
        at: (0., 0.),
        stroke: Some((6., B)),
    },
    Glyph {
        d: "M4 4H19C34 4 42 15 42 32S34 60 19 60H4Z",
        at: (93., 9.),
        stroke: Some((6.5, Q)),
    },
    Glyph {
        d: "M4 4V41C4 54 10 61 22 61S40 54 40 41V4",
        at: (150., 9.),
        stroke: Some((6.5, Q)),
    },
    Glyph {
        d: "M40 11C36 5 30 3 22 3C9 3 3 12 3 25V39C3 52 9 61 22 61C30 61 36 58 40 53",
        at: (207., 9.),
        stroke: Some((6.5, Q)),
    },
    Glyph {
        d: "M4 4V60 M8 30.470588L38 4 M12 27L40 60",
        at: (264., 9.),
        stroke: Some((6.5, Q)),
    },
    Glyph {
        d: "M4 4H19C34 4 42 15 42 32S34 60 19 60H4Z",
        at: (321., 9.),
        stroke: Some((6.5, Q)),
    },
    Glyph {
        d: "M39 10C35 5 29 3 21 3C9 3 4 9 4 18C4 38 40 23 40 45C40 55 33 61 21 61C13 61 7 58 3 53",
        at: (378., 9.),
        stroke: Some((6.5, Q)),
    },
    Glyph {
        d: "M4 60V4H22C35 4 41 10 41 21S35 38 22 38H4",
        at: (435., 9.),
        stroke: Some((6.5, Q)),
    },
];

impl Brand {
    /// The artwork's view box, in its own units.
    pub(crate) fn view_box(self) -> (f32, f32) {
        match self {
            Self::DuckDsp => (492., 84.),
        }
    }

    /// Width for a given height, keeping the proportions.
    pub(crate) fn width_for(self, height: f32) -> f32 {
        let (w, h) = self.view_box();
        height * w / h
    }

    fn glyphs(self) -> &'static [Glyph] {
        match self {
            Self::DuckDsp => &DUCKDSP,
        }
    }

    fn paths(self) -> &'static [(Path, Option<Stroke>)] {
        static D: OnceLock<Vec<(Path, Option<Stroke>)>> = OnceLock::new();
        let build = || {
            self.glyphs()
                .iter()
                .map(|g| {
                    let path = parse(g.d, g.at).expect("valid embedded brand path");
                    let stroke = g.stroke.map(|(width, line_cap)| Stroke {
                        width,
                        line_cap,
                        line_join: LineJoin::Miter,
                        miter_limit: 4.0,
                        ..Stroke::default()
                    });
                    (path, stroke)
                })
                .collect()
        };
        match self {
            Self::DuckDsp => D.get_or_init(build),
        }
    }
}

/// Parse the absolute SVG subset the brand files use.
fn parse(d: &str, (dx, dy): (f32, f32)) -> Option<Path> {
    let mut tokens = Vec::new();
    let mut num = String::new();
    let flush = |num: &mut String, tokens: &mut Vec<Tok>| -> Option<()> {
        if !num.is_empty() {
            tokens.push(Tok::N(num.parse().ok()?));
            num.clear();
        }
        Some(())
    };
    for c in d.chars() {
        if c.is_ascii_alphabetic() {
            flush(&mut num, &mut tokens)?;
            tokens.push(Tok::C(c));
        } else if c == '-' && !num.is_empty() {
            flush(&mut num, &mut tokens)?;
            num.push(c);
        } else if c.is_ascii_digit() || c == '.' || c == '-' {
            num.push(c);
        } else {
            flush(&mut num, &mut tokens)?;
        }
    }
    flush(&mut num, &mut tokens)?;

    let mut p = PathBuilder::new();
    let (mut x, mut y) = (0.0_f32, 0.0_f32);
    let mut last_c2: Option<(f32, f32)> = None;
    let mut i = 0;
    let mut cmd = ' ';
    let take = |i: &mut usize, n: usize| -> Option<Vec<f32>> {
        let v: Option<Vec<f32>> = tokens
            .get(*i..*i + n)?
            .iter()
            .map(|t| if let Tok::N(v) = t { Some(*v) } else { None })
            .collect();
        *i += n;
        v
    };
    while i < tokens.len() {
        if let Tok::C(c) = tokens[i] {
            cmd = c;
            i += 1;
            if c == 'Z' {
                p.close();
                last_c2 = None;
                continue;
            }
        }
        let prev = last_c2.take();
        match cmd {
            'M' | 'L' => {
                let v = take(&mut i, 2)?;
                (x, y) = (v[0], v[1]);
                if cmd == 'M' {
                    p.move_to(x + dx, y + dy);
                    cmd = 'L';
                } else {
                    p.line_to(x + dx, y + dy);
                }
            }
            'H' => {
                x = take(&mut i, 1)?[0];
                p.line_to(x + dx, y + dy);
            }
            'V' => {
                y = take(&mut i, 1)?[0];
                p.line_to(x + dx, y + dy);
            }
            'C' | 'S' => {
                let (c1, rest) = if cmd == 'C' {
                    let v = take(&mut i, 6)?;
                    ((v[0], v[1]), v[2..].to_vec())
                } else {
                    let r = prev.map_or((x, y), |(px, py)| (2. * x - px, 2. * y - py));
                    (r, take(&mut i, 4)?)
                };
                p.cubic_to(
                    c1.0 + dx,
                    c1.1 + dy,
                    rest[0] + dx,
                    rest[1] + dy,
                    rest[2] + dx,
                    rest[3] + dy,
                );
                last_c2 = Some((rest[0], rest[1]));
                (x, y) = (rest[2], rest[3]);
            }
            _ => return None,
        }
    }
    p.finish()
}

#[derive(Clone, Copy)]
enum Tok {
    C(char),
    N(f32),
}

#[derive(Clone)]
struct Cached {
    size: [u32; 2],
    texture: TextureHandle,
}

/// Paint `brand` into `rect` (its proportions), tinted `tint`.
pub(crate) fn paint(ui: &Ui, brand: Brand, rect: Rect, tint: Color32) {
    // Twice the physical size keeps the lettering crisp at every zoom.
    let (vw, vh) = brand.view_box();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let width = (rect.width() * ui.ctx().pixels_per_point() * 2.)
        .ceil()
        .clamp(1., 4096.) as u32;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let height = (width as f32 * vh / vw).ceil().max(1.) as u32;
    let size = [width, height];
    let id = egui::Id::new(("duckdsp-brand-texture", brand));
    let mut cached = ui.ctx().data_mut(|d| d.get_temp::<Cached>(id));
    if cached.as_ref().is_none_or(|c| c.size != size) {
        let Some(pixels) = rasterize(brand, width, height) else {
            return;
        };
        let image = egui::ColorImage::from_rgba_premultiplied(
            [width as usize, height as usize],
            pixels.data(),
        );
        let entry = Cached {
            size,
            texture: ui
                .ctx()
                .load_texture("duckdsp-brand", image, TextureOptions::LINEAR),
        };
        ui.ctx().data_mut(|d| d.insert_temp(id, entry.clone()));
        cached = Some(entry);
    }
    if let Some(cached) = cached {
        ui.painter().image(
            cached.texture.id(),
            rect,
            Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)),
            tint,
        );
    }
}

fn rasterize(brand: Brand, width: u32, height: u32) -> Option<Pixmap> {
    let mut pixels = Pixmap::new(width, height)?;
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    #[allow(clippy::cast_precision_loss)]
    let scale = width as f32 / brand.view_box().0;
    let t = Transform::from_scale(scale, scale);
    for (path, stroke) in brand.paths() {
        match stroke {
            Some(s) => pixels.stroke_path(path, &paint, s, t, None),
            None => pixels.fill_path(path, &paint, FillRule::Winding, t, None),
        }
    }
    Some(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_outline_parses_and_draws_ink() {
        let brand = Brand::DuckDsp;
        assert!(!brand.paths().is_empty());
        let px = rasterize(brand, 400, 120).expect("pixmap");
        let ink = px.data().chunks(4).filter(|p| p[3] > 0).count();
        assert!(ink > 1000, "{brand:?}: {ink}");
    }

    #[test]
    fn smooth_curve_reflects_the_last_control_point() {
        let p = parse("M0 0C0 10 10 10 10 0S20 -10 20 0", (0., 0.)).expect("path");
        let b = p.bounds();
        assert!(b.top() < -4.0 && b.bottom() > 4.0);
    }
}
