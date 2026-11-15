//! Vello backends for the shared [`Draw`](crate::Draw) commands.
//!
//! `VelloTarget` records into a `vello::Scene` that a wgpu device can render.
//! `render_vello_cpu` rasterizes the same commands with `vello_cpu` when no
//! GPU adapter is available. The software [`Canvas`](crate::Canvas) remains
//! the other fallback, including the headless screenshot path.

use std::sync::Arc;

use mithu_base::{Color, Rect};
use mithu_image::Bitmap;
use mithu_text::{FontBook, MeasureText, glyph_top};
use vello::peniko::{Blob, Fill, ImageAlphaType, ImageData, ImageFormat};
use vello::kurbo::{Affine, BezPath, Circle, Rect as KRect, RoundedRect};
use vello::Scene;
use vello_cpu::kurbo::{Affine as CpuAffine, BezPath as CpuPath, Circle as CpuCircle, Rect as CpuRect, Shape as CpuShape};
use vello_cpu::peniko::{
    Extend, Fill as CpuFill, ImageAlphaType as CpuAlpha, ImageQuality, ImageSampler,
};
use vello_cpu::{Image, ImageSource, Pixmap, RenderContext, Resources};

use crate::{Draw, intersect};

pub struct VelloTarget<'a> {
    scene: &'a mut Scene,
    width: i32,
    height: i32,
}

impl<'a> VelloTarget<'a> {
    pub fn new(scene: &'a mut Scene, width: i32, height: i32) -> Self {
        Self {
            scene,
            width,
            height,
        }
    }
}

fn gpu_color(color: Color) -> vello::peniko::Color {
    vello::peniko::Color::from_rgba8(color.r, color.g, color.b, color.a)
}

fn krect(rect: Rect) -> KRect {
    KRect::new(
        rect.x as f64,
        rect.y as f64,
        rect.right() as f64,
        rect.bottom() as f64,
    )
}

fn rgba_straight(bitmap: &Bitmap) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(bitmap.pixels.len() * 4);
    for pixel in &bitmap.pixels {
        bytes.push((pixel >> 16) as u8);
        bytes.push((pixel >> 8) as u8);
        bytes.push(*pixel as u8);
        bytes.push((pixel >> 24) as u8);
    }
    bytes
}

fn image_data(bitmap: &Bitmap) -> ImageData {
    ImageData {
        data: Blob::new(Arc::new(rgba_straight(bitmap))),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: bitmap.width,
        height: bitmap.height,
    }
}

fn tinted(bitmap: &Bitmap, tint: Color) -> Bitmap {
    let pixels = bitmap
        .pixels
        .iter()
        .map(|pixel| {
            let alpha = ((pixel >> 24) as u32 * u32::from(tint.a) / 255) as u8;
            (u32::from(alpha) << 24)
                | (u32::from(tint.r) << 16)
                | (u32::from(tint.g) << 8)
                | u32::from(tint.b)
        })
        .collect();
    Bitmap {
        width: bitmap.width,
        height: bitmap.height,
        pixels,
    }
}

fn place_glyphs(
    fonts: &FontBook,
    x: f32,
    baseline: f32,
    text: &str,
    size: f32,
    bold: bool,
    mono: bool,
    family: &str,
    slot_width: Option<f32>,
    mut stamp: impl FnMut(i32, i32, &mithu_text::Glyph),
) {
    if text.is_empty() || size <= 0.0 {
        return;
    }
    let glyphs = fonts.shape(text, size, bold, mono, family);
    let shaped: f32 = glyphs.iter().map(|glyph| glyph.advance).sum();
    let fit = match slot_width {
        Some(slot) if shaped > slot + 0.5 && slot > 0.0 => slot / shaped,
        _ => 1.0,
    };
    let mut pen = x;
    for glyph in &glyphs {
        if !pen.is_finite() || pen < -100_000.0 || pen > 100_000.0 {
            break;
        }
        let left = (pen.round() as i32)
            .saturating_add((glyph.xmin as f32 * fit).round() as i32)
            .saturating_add((glyph.x_offset * fit).round() as i32);
        let top = glyph_top(baseline - glyph.y_offset, glyph);
        if glyph.width > 0 && glyph.height > 0 && !glyph.coverage.is_empty() {
            stamp(left, top, glyph);
        }
        pen += glyph.advance * fit;
    }
}

impl Draw for VelloTarget<'_> {
    fn width(&self) -> i32 {
        self.width
    }
    fn height(&self) -> i32 {
        self.height
    }

    fn fill(&mut self, rect: Rect, color: Color, clip: Rect) {
        self.fill_round(rect, color, 0.0, clip);
    }

    fn fill_vertical(&mut self, rect: Rect, top: Color, bottom: Color, clip: Rect) {
        let Some(rect) = intersect(rect, clip) else {
            return;
        };
        let bands = 24;
        let band = rect.height / bands as f32;
        for index in 0..bands {
            let t = index as f32 / (bands - 1) as f32;
            let color = mix(top, bottom, t);
            let y = rect.y + band * index as f32;
            self.scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                gpu_color(color),
                None,
                &krect(Rect::new(rect.x, y, rect.width, band + 0.5)),
            );
        }
    }

    fn fill_round(&mut self, rect: Rect, color: Color, radius: f32, clip: Rect) {
        if color.a == 0 || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let Some(visible) = intersect(rect, clip) else {
            return;
        };
        let shape_rect = if radius <= 0.0 { visible } else { rect };
        let shape = krect(shape_rect);
        let rounded = radius > 0.0;
        if rounded && (visible.x > rect.x + 0.5 || visible.right() < rect.right() - 0.5
            || visible.y > rect.y + 0.5 || visible.bottom() < rect.bottom() - 0.5)
        {
            self.scene
                .push_clip_layer(Fill::NonZero, Affine::IDENTITY, &krect(visible));
        }
        if rounded {
            let radius = radius.min(rect.width / 2.0).min(rect.height / 2.0) as f64;
            let round = RoundedRect::from_rect(shape, radius);
            self.scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                gpu_color(color),
                None,
                &round,
            );
        } else {
            self.scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                gpu_color(color),
                None,
                &shape,
            );
        }
        if rounded && (visible.x > rect.x + 0.5 || visible.right() < rect.right() - 0.5
            || visible.y > rect.y + 0.5 || visible.bottom() < rect.bottom() - 0.5)
        {
            self.scene.pop_layer();
        }
    }

    fn fill_contours(
        &mut self,
        contours: &[Vec<(f32, f32)>],
        color: Color,
        clip: Rect,
        even_odd: bool,
    ) {
        if color.a == 0 || contours.is_empty() {
            return;
        }
        let mut path = BezPath::new();
        for contour in contours {
            if contour.len() < 2 {
                continue;
            }
            path.move_to((contour[0].0 as f64, contour[0].1 as f64));
            for point in contour.iter().skip(1) {
                path.line_to((point.0 as f64, point.1 as f64));
            }
            path.close_path();
        }
        if path.is_empty() {
            return;
        }
        let fill = if even_odd { Fill::EvenOdd } else { Fill::NonZero };
        self.scene
            .push_clip_layer(Fill::NonZero, Affine::IDENTITY, &krect(clip));
        self.scene
            .fill(fill, Affine::IDENTITY, gpu_color(color), None, &path);
        self.scene.pop_layer();
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color, clip: Rect) {
        if radius <= 0.0 || color.a == 0 {
            return;
        }
        self.scene
            .push_clip_layer(Fill::NonZero, Affine::IDENTITY, &krect(clip));
        self.scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            gpu_color(color),
            None,
            &Circle::new((cx as f64, cy as f64), radius as f64),
        );
        self.scene.pop_layer();
    }

    fn mask(&mut self, dest: Rect, bitmap: &Bitmap, tint: Color, clip: Rect) {
        let tinted = tinted(bitmap, tint);
        self.image(dest, &tinted, clip);
    }

    fn image(&mut self, dest: Rect, bitmap: &Bitmap, clip: Rect) {
        let Some((dest, bitmap)) = visible_bitmap(dest, bitmap, clip) else {
            return;
        };
        let data = image_data(&bitmap);
        let sx = dest.width as f64 / bitmap.width as f64;
        let sy = dest.height as f64 / bitmap.height as f64;
        let transform = Affine::translate((dest.x as f64, dest.y as f64))
            * Affine::scale_non_uniform(sx, sy);
        self.scene.draw_image(&data, transform);
    }

    fn text(
        &mut self,
        fonts: &FontBook,
        x: f32,
        baseline: f32,
        text: &str,
        size: f32,
        color: Color,
        bold: bool,
        mono: bool,
        underline: bool,
        clip: Rect,
    ) {
        self.text_fitted(
            fonts, x, baseline, text, size, color, bold, mono, underline, "", 0.0, clip,
        );
    }

    fn text_family(
        &mut self,
        fonts: &FontBook,
        x: f32,
        baseline: f32,
        text: &str,
        size: f32,
        color: Color,
        bold: bool,
        mono: bool,
        underline: bool,
        family: &str,
        clip: Rect,
    ) {
        self.text_fitted(
            fonts, x, baseline, text, size, color, bold, mono, underline, family, 0.0, clip,
        );
    }

    fn text_fitted(
        &mut self,
        fonts: &FontBook,
        x: f32,
        baseline: f32,
        text: &str,
        size: f32,
        color: Color,
        bold: bool,
        mono: bool,
        underline: bool,
        family: &str,
        slot_width: f32,
        clip: Rect,
    ) {
        if color.a == 0 {
            return;
        }
        place_glyphs(
            fonts,
            x,
            baseline,
            text,
            size,
            bold,
            mono,
            family,
            (slot_width > 0.0).then_some(slot_width),
            |left, top, glyph| {
                let bounds = Rect::new(left as f32, top as f32, glyph.width as f32, glyph.height as f32);
                if intersect(bounds, clip).is_none() {
                    return;
                }
                let mut pixels = Vec::with_capacity(glyph.coverage.len());
                for &coverage in &glyph.coverage {
                    let alpha = (u32::from(coverage) * u32::from(color.a) / 255) as u8;
                    pixels.push(
                        (u32::from(alpha) << 24)
                            | (u32::from(color.r) << 16)
                            | (u32::from(color.g) << 8)
                            | u32::from(color.b),
                    );
                }
                let bitmap = Bitmap {
                    width: glyph.width as u32,
                    height: glyph.height as u32,
                    pixels,
                };
                self.image(bounds, &bitmap, clip);
            },
        );
        if underline {
            let width = if slot_width > 0.0 {
                slot_width
            } else {
                fonts.width_family(text, size, bold, mono, family)
            };
            let y = baseline + (size * 0.12).max(1.0);
            self.fill(Rect::new(x, y, width, (size * 0.06).max(1.0)), color, clip);
        }
    }
}

fn mix(top: Color, bottom: Color, t: f32) -> Color {
    let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color {
        r: lerp(top.r, bottom.r),
        g: lerp(top.g, bottom.g),
        b: lerp(top.b, bottom.b),
        a: lerp(top.a, bottom.a),
    }
}

struct CpuTarget<'a> {
    ctx: &'a mut RenderContext,
    width: i32,
    height: i32,
}

fn cpu_color(color: Color) -> vello_cpu::peniko::Color {
    vello_cpu::peniko::Color::from_rgba8(color.r, color.g, color.b, color.a)
}

fn cpu_rect(rect: Rect) -> CpuRect {
    CpuRect::new(
        rect.x as f64,
        rect.y as f64,
        rect.right() as f64,
        rect.bottom() as f64,
    )
}

impl Draw for CpuTarget<'_> {
    fn width(&self) -> i32 {
        self.width
    }
    fn height(&self) -> i32 {
        self.height
    }

    fn fill(&mut self, rect: Rect, color: Color, clip: Rect) {
        let Some(rect) = intersect(rect, clip) else {
            return;
        };
        self.ctx.set_paint(cpu_color(color));
        self.ctx.set_fill_rule(CpuFill::NonZero);
        self.ctx.fill_rect(&cpu_rect(rect));
    }

    fn fill_vertical(&mut self, rect: Rect, top: Color, bottom: Color, clip: Rect) {
        let Some(rect) = intersect(rect, clip) else {
            return;
        };
        let bands = 24;
        let band = rect.height / bands as f32;
        for index in 0..bands {
            let t = index as f32 / (bands - 1) as f32;
            self.fill(
                Rect::new(rect.x, rect.y + band * index as f32, rect.width, band + 0.5),
                mix(top, bottom, t),
                rect,
            );
        }
    }

    fn fill_round(&mut self, rect: Rect, color: Color, radius: f32, clip: Rect) {
        if radius <= 0.0 {
            self.fill(rect, color, clip);
            return;
        }
        let Some(visible) = intersect(rect, clip) else {
            return;
        };
        self.ctx.set_paint(cpu_color(color));
        self.ctx.set_fill_rule(CpuFill::NonZero);
        self.ctx
            .push_clip_layer(&cpu_rect(visible).to_path(0.1));
        let radius = radius.min(rect.width / 2.0).min(rect.height / 2.0) as f64;
        let round = vello_cpu::kurbo::RoundedRect::from_rect(cpu_rect(rect), radius);
        self.ctx.fill_path(&round.to_path(0.1));
        self.ctx.pop_layer();
    }

    fn fill_contours(
        &mut self,
        contours: &[Vec<(f32, f32)>],
        color: Color,
        clip: Rect,
        even_odd: bool,
    ) {
        if color.a == 0 || contours.is_empty() {
            return;
        }
        let mut path = CpuPath::new();
        for contour in contours {
            if contour.len() < 2 {
                continue;
            }
            path.move_to((contour[0].0 as f64, contour[0].1 as f64));
            for point in contour.iter().skip(1) {
                path.line_to((point.0 as f64, point.1 as f64));
            }
            path.close_path();
        }
        if path.is_empty() {
            return;
        }
        self.ctx.set_paint(cpu_color(color));
        self.ctx
            .set_fill_rule(if even_odd { CpuFill::EvenOdd } else { CpuFill::NonZero });
        self.ctx.push_clip_layer(&cpu_rect(clip).to_path(0.1));
        self.ctx.fill_path(&path);
        self.ctx.pop_layer();
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color, clip: Rect) {
        if radius <= 0.0 || color.a == 0 {
            return;
        }
        self.ctx.set_paint(cpu_color(color));
        self.ctx.set_fill_rule(CpuFill::NonZero);
        self.ctx.push_clip_layer(&cpu_rect(clip).to_path(0.1));
        self.ctx.fill_path(
            &CpuCircle::new((cx as f64, cy as f64), radius as f64).to_path(0.1),
        );
        self.ctx.pop_layer();
    }

    fn mask(&mut self, dest: Rect, bitmap: &Bitmap, tint: Color, clip: Rect) {
        let tinted = tinted(bitmap, tint);
        self.image(dest, &tinted, clip);
    }

    fn image(&mut self, dest: Rect, bitmap: &Bitmap, clip: Rect) {
        let Some((dest, bitmap)) = visible_bitmap(dest, bitmap, clip) else {
            return;
        };
        let Some(pixmap) = bitmap_pixmap(&bitmap) else {
            return;
        };
        self.ctx.set_paint(Image {
            image: ImageSource::Pixmap(Arc::new(pixmap)),
            sampler: ImageSampler {
                x_extend: Extend::Pad,
                y_extend: Extend::Pad,
                quality: ImageQuality::Low,
                alpha: 1.0,
            },
        });
        let sx = dest.width as f64 / bitmap.width as f64;
        let sy = dest.height as f64 / bitmap.height as f64;
        self.ctx.set_transform(
            CpuAffine::translate((dest.x as f64, dest.y as f64))
                * CpuAffine::scale_non_uniform(sx, sy),
        );
        self.ctx.fill_rect(&CpuRect::new(
            0.0,
            0.0,
            bitmap.width as f64,
            bitmap.height as f64,
        ));
        self.ctx.reset_transform();
    }

    fn text(
        &mut self,
        fonts: &FontBook,
        x: f32,
        baseline: f32,
        text: &str,
        size: f32,
        color: Color,
        bold: bool,
        mono: bool,
        underline: bool,
        clip: Rect,
    ) {
        self.text_fitted(
            fonts, x, baseline, text, size, color, bold, mono, underline, "", 0.0, clip,
        );
    }

    fn text_family(
        &mut self,
        fonts: &FontBook,
        x: f32,
        baseline: f32,
        text: &str,
        size: f32,
        color: Color,
        bold: bool,
        mono: bool,
        underline: bool,
        family: &str,
        clip: Rect,
    ) {
        self.text_fitted(
            fonts, x, baseline, text, size, color, bold, mono, underline, family, 0.0, clip,
        );
    }

    fn text_fitted(
        &mut self,
        fonts: &FontBook,
        x: f32,
        baseline: f32,
        text: &str,
        size: f32,
        color: Color,
        bold: bool,
        mono: bool,
        underline: bool,
        family: &str,
        slot_width: f32,
        clip: Rect,
    ) {
        if color.a == 0 {
            return;
        }
        place_glyphs(
            fonts,
            x,
            baseline,
            text,
            size,
            bold,
            mono,
            family,
            (slot_width > 0.0).then_some(slot_width),
            |left, top, glyph| {
                let bounds = Rect::new(left as f32, top as f32, glyph.width as f32, glyph.height as f32);
                if intersect(bounds, clip).is_none() {
                    return;
                }
                let mut pixels = Vec::with_capacity(glyph.coverage.len());
                for &coverage in &glyph.coverage {
                    let alpha = (u32::from(coverage) * u32::from(color.a) / 255) as u8;
                    pixels.push(
                        (u32::from(alpha) << 24)
                            | (u32::from(color.r) << 16)
                            | (u32::from(color.g) << 8)
                            | u32::from(color.b),
                    );
                }
                let bitmap = Bitmap {
                    width: glyph.width as u32,
                    height: glyph.height as u32,
                    pixels,
                };
                self.image(bounds, &bitmap, clip);
            },
        );
        if underline {
            let width = if slot_width > 0.0 {
                slot_width
            } else {
                fonts.width_family(text, size, bold, mono, family)
            };
            let y = baseline + (size * 0.12).max(1.0);
            self.fill(Rect::new(x, y, width, (size * 0.06).max(1.0)), color, clip);
        }
    }
}

/// Crop `bitmap` to the part of `dest` that lies inside `clip`.
/// A clip layer on top of an image transform was sampling the wrong texels,
/// which scrambled chrome text while page text (a loose clip) stayed intact.
fn visible_bitmap(dest: Rect, bitmap: &Bitmap, clip: Rect) -> Option<(Rect, Bitmap)> {
    if dest.width <= 0.0 || dest.height <= 0.0 || bitmap.width == 0 || bitmap.height == 0 {
        return None;
    }
    let visible = intersect(dest, clip)?;
    let u0 = ((visible.x - dest.x) / dest.width * bitmap.width as f32)
        .floor()
        .clamp(0.0, bitmap.width as f32 - 1.0) as u32;
    let v0 = ((visible.y - dest.y) / dest.height * bitmap.height as f32)
        .floor()
        .clamp(0.0, bitmap.height as f32 - 1.0) as u32;
    let u1 = ((visible.right() - dest.x) / dest.width * bitmap.width as f32)
        .ceil()
        .clamp(u0 as f32 + 1.0, bitmap.width as f32) as u32;
    let v1 = ((visible.bottom() - dest.y) / dest.height * bitmap.height as f32)
        .ceil()
        .clamp(v0 as f32 + 1.0, bitmap.height as f32) as u32;
    let width = u1 - u0;
    let height = v1 - v0;
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in v0..v1 {
        let row = (y * bitmap.width) as usize;
        pixels.extend_from_slice(&bitmap.pixels[row + u0 as usize..row + u1 as usize]);
    }
    let cropped_dest = Rect::new(
        dest.x + u0 as f32 * dest.width / bitmap.width as f32,
        dest.y + v0 as f32 * dest.height / bitmap.height as f32,
        width as f32 * dest.width / bitmap.width as f32,
        height as f32 * dest.height / bitmap.height as f32,
    );
    Some((
        cropped_dest,
        Bitmap {
            width,
            height,
            pixels,
        },
    ))
}

fn bitmap_pixmap(bitmap: &Bitmap) -> Option<Pixmap> {
    if bitmap.width == 0
        || bitmap.height == 0
        || bitmap.width > u32::from(u16::MAX)
        || bitmap.height > u32::from(u16::MAX)
    {
        return None;
    }
    let mut pixels = Vec::with_capacity(bitmap.pixels.len() * 4);
    for pixel in &bitmap.pixels {
        let a = (pixel >> 24) as u8;
        let r = (pixel >> 16) as u8;
        let g = (pixel >> 8) as u8;
        let b = *pixel as u8;
        // Premultiplied transparent pixels must be zero. A straight-alpha
        // texel with rgb set and alpha 0 is read back as a dark smear.
        let (r, g, b) = if a == 0 {
            (0, 0, 0)
        } else if a == 255 {
            (r, g, b)
        } else {
            (
                (u16::from(r) * u16::from(a) / 255) as u8,
                (u16::from(g) * u16::from(a) / 255) as u8,
                (u16::from(b) * u16::from(a) / 255) as u8,
            )
        };
        pixels.extend_from_slice(&[r, g, b, a]);
    }
    Some(Pixmap::from_parts(
        pixels,
        bitmap.width as u16,
        bitmap.height as u16,
        vello_cpu::PixelMetadata::new(CpuAlpha::AlphaPremultiplied, true),
    ))
}

/// Rasterize `draw` with vello_cpu. Returns packed `0x00RRGGBB` pixels.
pub fn render_vello_cpu(
    width: u32,
    height: u32,
    draw: impl FnOnce(&mut dyn Draw),
) -> Option<Vec<u32>> {
    if width == 0 || height == 0 || width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return None;
    }
    let mut ctx = RenderContext::new(width as u16, height as u16);
    {
        let mut target = CpuTarget {
            ctx: &mut ctx,
            width: width as i32,
            height: height as i32,
        };
        draw(&mut target);
    }
    ctx.flush();
    let mut pixmap = Pixmap::new(width as u16, height as u16);
    let mut resources = Resources::new();
    ctx.render(&mut pixmap, &mut resources);
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for px in pixmap.data() {
        let (r, g, b, a) = (px.r, px.g, px.b, px.a);
        let (r, g, b) = if a == 0 {
            (0, 0, 0)
        } else if a == 255 {
            (r, g, b)
        } else {
            (
                (u16::from(r) * 255 / u16::from(a)).min(255) as u8,
                (u16::from(g) * 255 / u16::from(a)).min(255) as u8,
                (u16::from(b) * 255 / u16::from(a)).min(255) as u8,
            )
        };
        pixels.push((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b));
    }
    Some(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Canvas;
    use mithu_base::Color;
    use mithu_text::FontBook;

    #[test]
    fn cpu_text_stays_near_the_software_painter() {
        let fonts = FontBook::load();
        let width = 220u32;
        let height = 36u32;
        let paint = |draw: &mut dyn Draw| {
            let clip = Rect::new(0.0, 0.0, width as f32, height as f32);
            draw.fill(clip, Color::WHITE, clip);
            draw.text(
                &fonts,
                8.0,
                24.0,
                "Example",
                13.0,
                Color::rgb(20, 20, 20),
                true,
                false,
                false,
                clip,
            );
        };
        let vello = render_vello_cpu(width, height, paint).expect("vello");
        let mut pixels = vec![0x00FF_FFFFu32; (width * height) as usize];
        {
            let mut canvas = Canvas {
                pixels: &mut pixels,
                width: width as i32,
                height: height as i32,
            };
            paint(&mut canvas);
        }
        let mut dark_cpu = 0u32;
        let mut dark_vello = 0u32;
        let mut mismatch = 0u32;
        for (cpu, gpu) in pixels.iter().zip(vello.iter()) {
            if cpu & 0x00FF_FFFF != 0x00FF_FFFF {
                dark_cpu += 1;
            }
            if gpu & 0x00FF_FFFF != 0x00FF_FFFF {
                dark_vello += 1;
            }
            if cpu != gpu {
                mismatch += 1;
            }
        }
        assert!(dark_cpu > 20, "cpu ink {dark_cpu}");
        assert!(
            dark_vello + 120 > dark_cpu && dark_cpu + 120 > dark_vello,
            "ink cpu {dark_cpu} vello {dark_vello} mismatch {mismatch}"
        );
        let clipped = |draw: &mut dyn Draw| {
            draw.fill(
                Rect::new(0.0, 0.0, width as f32, height as f32),
                Color::WHITE,
                Rect::new(0.0, 0.0, width as f32, height as f32),
            );
            let clip = Rect::new(8.0, 8.0, 120.0, 20.0);
            draw.text(
                &fonts,
                8.0,
                24.0,
                "Example",
                13.0,
                Color::rgb(20, 20, 20),
                true,
                false,
                false,
                clip,
            );
        };
        let vello = render_vello_cpu(width, height, clipped).expect("vello");
        let mut pixels = vec![0x00FF_FFFFu32; (width * height) as usize];
        {
            let mut canvas = Canvas {
                pixels: &mut pixels,
                width: width as i32,
                height: height as i32,
            };
            clipped(&mut canvas);
        }
        let mut dark_cpu = 0u32;
        let mut dark_vello = 0u32;
        for (cpu, gpu) in pixels.iter().zip(vello.iter()) {
            if cpu & 0x00FF_FFFF != 0x00FF_FFFF {
                dark_cpu += 1;
            }
            if gpu & 0x00FF_FFFF != 0x00FF_FFFF {
                dark_vello += 1;
            }
        }
        assert!(
            dark_vello + 120 > dark_cpu && dark_cpu + 120 > dark_vello,
            "clipped ink cpu {dark_cpu} vello {dark_vello}"
        );
        let word = |draw: &mut dyn Draw| {
            let full = Rect::new(0.0, 0.0, 80.0, 32.0);
            draw.fill(full, Color::WHITE, full);
            draw.text(
                &fonts,
                4.0,
                22.0,
                "Hi",
                16.0,
                Color::rgb(0, 0, 0),
                false,
                false,
                false,
                full,
            );
        };
        let vello = render_vello_cpu(80, 32, word).expect("hi");
        let mut cpu = vec![0x00FF_FFFFu32; 80 * 32];
        {
            let mut canvas = Canvas {
                pixels: &mut cpu,
                width: 80,
                height: 32,
            };
            word(&mut canvas);
        }
        let mut mismatch = 0u32;
        for (cpu, gpu) in cpu.iter().zip(vello.iter()) {
            if cpu != gpu {
                mismatch += 1;
            }
        }
        assert!(mismatch < 40, "Hi mismatch {mismatch}");

        // Colored glyphs (chrome URL ink) must not smear their rgb into
        // transparent texels. A tight clip after a rounded fill is the
        // omnibox case.
        let chrome = |draw: &mut dyn Draw| {
            let full = Rect::new(0.0, 0.0, 360.0, 48.0);
            draw.fill(full, Color::rgb(232, 234, 237), full);
            draw.fill_round(
                Rect::new(8.0, 6.0, 340.0, 36.0),
                Color::rgb(255, 255, 255),
                18.0,
                full,
            );
            let clip = Rect::new(24.0, 10.0, 300.0, 28.0);
            draw.text(
                &fonts,
                24.0,
                30.0,
                "https://example.com",
                16.0,
                Color::rgb(30, 30, 30),
                false,
                false,
                false,
                clip,
            );
        };
        let vello = render_vello_cpu(360, 48, chrome).expect("chrome");
        let mut cpu = vec![0x00FF_FFFFu32; 360 * 48];
        {
            let mut canvas = Canvas {
                pixels: &mut cpu,
                width: 360,
                height: 48,
            };
            chrome(&mut canvas);
        }
        let dark = |pixel: u32| (pixel & 0xFF) < 160 && ((pixel >> 8) & 0xFF) < 160;
        let mut mismatch = 0u32;
        for (cpu, gpu) in cpu.iter().zip(vello.iter()) {
            if dark(*cpu) != dark(*gpu) {
                mismatch += 1;
            }
        }
        assert!(mismatch < 80, "chrome url mismatch {mismatch}");
    }

    #[test]
    fn cpu_fallback_fills_a_rect() {
        let pixels = render_vello_cpu(32, 32, |draw| {
            let full = Rect::new(0.0, 0.0, 32.0, 32.0);
            draw.fill(Rect::new(4.0, 4.0, 10.0, 10.0), Color::rgb(255, 0, 0), full);
        })
        .expect("vello_cpu");
        let red = pixels[8 * 32 + 8];
        assert_eq!(red & 0x00FF_0000, 0x00FF_0000, "pixel {red:#08x}");
        assert_eq!(pixels[0], 0);
    }
}
