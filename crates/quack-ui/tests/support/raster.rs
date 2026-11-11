#![allow(dead_code)] // shared copy of Sonder's rasteriser; not every helper is used here
//! Software renderer for egui output, for tests and the design
//! captures only (never in the plugin).
//!
//! egui tessellates everything into textured triangles with
//! premultiplied sRGB vertex colours and feathered edges, so a plain
//! triangle filler that blends in gamma space, like egui's own GPU
//! backends, reproduces the editor.

use std::collections::HashMap;

use egui::epaint::{ClippedPrimitive, ImageDelta, Primitive, TextureId, Vertex};
use egui::{Color32, ImageData};

/// An RGBA image, premultiplied alpha, row by row.
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[u8; 4]>,
}

/// Texture store fed by egui's `textures_delta`.
#[derive(Default)]
pub struct Textures {
    images: HashMap<TextureId, (usize, usize, Vec<Color32>)>,
}

impl Textures {
    pub fn apply(&mut self, set: &[(TextureId, ImageDelta)]) {
        for (id, delta) in set {
            let ImageData::Color(img) = &delta.image;
            let [w, h] = img.size;
            match delta.pos {
                None => {
                    self.images.insert(*id, (w, h, img.pixels.clone()));
                }
                Some([x0, y0]) => {
                    let (tw, _, px) = self.images.get_mut(id).expect("patch of a known texture");
                    for y in 0..h {
                        for x in 0..w {
                            px[(y0 + y) * *tw + x0 + x] = img.pixels[y * w + x];
                        }
                    }
                }
            }
        }
    }

    /// Bilinear sample, clamped at the edges (egui's font atlas uses
    /// linear filtering).
    fn sample(&self, id: TextureId, u: f32, v: f32) -> [f32; 4] {
        let Some((w, h, px)) = self.images.get(&id) else {
            return [1.0; 4];
        };
        let (w, h) = (*w, *h);
        let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |x: usize, y: usize| px[y * w + x].to_array().map(|c| f32::from(c) / 255.0);
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            top + (bottom - top) * fy
        })
    }
}

impl Canvas {
    pub fn new(width: usize, height: usize, fill: Color32) -> Self {
        Self {
            width,
            height,
            pixels: vec![fill.to_array(); width * height],
        }
    }

    pub fn get(&self, x: usize, y: usize) -> Color32 {
        let [r, g, b, a] = self.pixels[y * self.width + x];
        Color32::from_rgba_premultiplied(r, g, b, a)
    }

    /// Draw tessellated primitives. Positions are in points; one point
    /// is `pixels_per_point` pixels.
    pub fn paint(&mut self, prims: &[ClippedPrimitive], tex: &Textures, pixels_per_point: f32) {
        for p in prims {
            let Primitive::Mesh(mesh) = &p.primitive else {
                continue;
            };
            let clip = p.clip_rect;
            let cx0 = (clip.min.x * pixels_per_point).round().max(0.0) as usize;
            let cy0 = (clip.min.y * pixels_per_point).round().max(0.0) as usize;
            let cx1 = ((clip.max.x * pixels_per_point).round().max(0.0) as usize).min(self.width);
            let cy1 = ((clip.max.y * pixels_per_point).round().max(0.0) as usize).min(self.height);
            if cx0 >= cx1 || cy0 >= cy1 {
                continue;
            }
            for tri in mesh.indices.chunks_exact(3) {
                let v: [&Vertex; 3] = std::array::from_fn(|k| &mesh.vertices[tri[k] as usize]);
                self.triangle(
                    v,
                    mesh.texture_id,
                    tex,
                    pixels_per_point,
                    (cx0, cy0, cx1, cy1),
                );
            }
        }
    }

    fn triangle(
        &mut self,
        mut v: [&Vertex; 3],
        tid: TextureId,
        tex: &Textures,
        ppp: f32,
        (cx0, cy0, cx1, cy1): (usize, usize, usize, usize),
    ) {
        let pos = |v: &Vertex| (v.pos.x * ppp, v.pos.y * ppp);
        let mut p = [pos(v[0]), pos(v[1]), pos(v[2])];
        let mut area =
            (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
        if area.abs() < 1e-9 {
            return;
        }
        if area < 0.0 {
            // One orientation (clockwise on screen, y down) for the fill
            // rule below.
            v.swap(1, 2);
            p.swap(1, 2);
            area = -area;
        }
        let min_x = p.iter().map(|q| q.0).fold(f32::INFINITY, f32::min);
        let max_x = p.iter().map(|q| q.0).fold(f32::NEG_INFINITY, f32::max);
        let min_y = p.iter().map(|q| q.1).fold(f32::INFINITY, f32::min);
        let max_y = p.iter().map(|q| q.1).fold(f32::NEG_INFINITY, f32::max);
        let x0 = (min_x.floor().max(0.0) as usize).max(cx0);
        let x1 = ((max_x.ceil().max(0.0) as usize) + 1).min(cx1);
        let y0 = (min_y.floor().max(0.0) as usize).max(cy0);
        let y1 = ((max_y.ceil().max(0.0) as usize) + 1).min(cy1);
        let col = v.map(|v| v.color.to_array().map(|c| f32::from(c) / 255.0));
        let edge = |a: (f32, f32), b: (f32, f32), x: f32, y: f32| {
            (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)
        };
        // Top-left rule: a pixel centre exactly on an edge shared by two
        // triangles belongs to one of them only.
        let top_left = |a: (f32, f32), b: (f32, f32)| {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            (dy == 0.0 && dx > 0.0) || dy < 0.0
        };
        let owns = [
            top_left(p[1], p[2]),
            top_left(p[2], p[0]),
            top_left(p[0], p[1]),
        ];
        let inside = |e: f32, own: bool| e > 0.0 || (e == 0.0 && own);
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let e = [
                    edge(p[1], p[2], px, py),
                    edge(p[2], p[0], px, py),
                    edge(p[0], p[1], px, py),
                ];
                if !(0..3).all(|k| inside(e[k], owns[k])) {
                    continue;
                }
                let (w0, w1, w2) = (e[0] / area, e[1] / area, e[2] / area);
                let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                let t = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                let s = tex.sample(tid, u, t);
                let src: [f32; 4] = std::array::from_fn(|i| {
                    (w0 * col[0][i] + w1 * col[1][i] + w2 * col[2][i]) * s[i]
                });
                let dst = &mut self.pixels[y * self.width + x];
                let keep = 1.0 - src[3];
                for i in 0..4 {
                    let d = f32::from(dst[i]) / 255.0;
                    dst[i] = ((src[i] + d * keep).clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
    }

    /// Opaque RGB bytes (the captures have an opaque background).
    pub fn rgb(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect()
    }
}

/// Run `frames` frames of `ui` on a `w × h` point screen at
/// `pixels_per_point` and paint the last one.
pub fn render(
    ctx: &egui::Context,
    w: f32,
    h: f32,
    pixels_per_point: f32,
    frames: usize,
    background: Color32,
    draw: impl FnMut(&mut egui::Ui),
) -> Canvas {
    render_with(ctx, w, h, pixels_per_point, frames, background, false, draw)
}

/// A window of `w × h` pixels holding an editor that sets its own zoom
/// factor, as truce-egui runs it: each frame's screen is the window
/// divided by the current zoom.
pub fn render_window(
    ctx: &egui::Context,
    w: f32,
    h: f32,
    frames: usize,
    background: Color32,
    draw: impl FnMut(&mut egui::Ui),
) -> Canvas {
    render_with(ctx, w, h, 1.0, frames, background, true, draw)
}

#[allow(clippy::too_many_arguments)]
fn render_with(
    ctx: &egui::Context,
    w: f32,
    h: f32,
    pixels_per_point: f32,
    frames: usize,
    background: Color32,
    follow_zoom: bool,
    mut draw: impl FnMut(&mut egui::Ui),
) -> Canvas {
    let mut tex = Textures::default();
    let mut last = None;
    for i in 0..frames {
        let zoom = if follow_zoom && i > 0 {
            ctx.zoom_factor()
        } else {
            1.0
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(w, h) / zoom,
            )),
            time: Some(i as f64 / 60.0),
            ..Default::default()
        };
        if i == 0 {
            // Once: an editor that sets its own zoom factor keeps it.
            ctx.set_pixels_per_point(pixels_per_point);
        }
        let out = ctx.run_ui(input, |ui| draw(ui));
        tex.apply(&out.textures_delta.set);
        last = Some(out);
    }
    let out = last.expect("at least one frame");
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    let mut canvas = Canvas::new(
        (w * pixels_per_point).round() as usize,
        (h * pixels_per_point).round() as usize,
        background,
    );
    // Shapes are in points of the last frame's zoom.
    canvas.paint(&prims, &tex, out.pixels_per_point);
    canvas
}

/// Write `canvas` as an 8-bit RGB PNG.
pub fn write_png(canvas: &Canvas, path: &std::path::Path) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, canvas.width as u32, canvas.height as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    let mut w = enc.write_header().map_err(std::io::Error::other)?;
    w.write_image_data(&canvas.rgb())
        .map_err(std::io::Error::other)?;
    w.finish().map_err(std::io::Error::other)
}
