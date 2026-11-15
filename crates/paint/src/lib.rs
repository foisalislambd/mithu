//! Page painter. The software rasterizer packs pixels as `0x00RRGGBB`.
//! The same draw calls can be recorded into a Vello scene for wgpu, or into
//! vello_cpu when no GPU is available.
mod vello_scene;

use mithu_base::{Color, Rect};
use mithu_image::Bitmap;
use mithu_layout::{LayoutBox, LayoutTree};
use mithu_text::{FontBook, MeasureText, glyph_top};

pub struct Canvas<'a> {
    pub pixels: &'a mut [u32],
    pub width: i32,
    pub height: i32,
}

impl Canvas<'_> {
    pub fn fill(&mut self, rect: Rect, color: Color, clip: Rect) {
        self.fill_round(rect, color, 0.0, clip);
    }

    pub fn fill_vertical(&mut self, rect: Rect, top: Color, bottom: Color, clip: Rect) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let Some(rect) = intersect(rect, clip) else {
            return;
        };
        let x0 = rect.x.floor().max(0.0) as i32;
        let y0 = rect.y.floor().max(0.0) as i32;
        let x1 = rect.right().ceil().min(self.width as f32) as i32;
        let y1 = rect.bottom().ceil().min(self.height as f32) as i32;
        let span = (rect.height - 1.0).max(1.0);
        let stride = self.width as usize;
        for y in y0..y1 {
            let t = ((y as f32 - rect.y) / span).clamp(0.0, 1.0);
            let color = mix_t(top, bottom, t);
            let packed = color.to_u32();
            let row = y as usize * stride;
            for x in x0..x1 {
                let index = row + x as usize;
                self.pixels[index] = if color.a == 255 {
                    packed
                } else {
                    blend(self.pixels[index], color, 255)
                };
            }
        }
    }

    pub fn fill_round(&mut self, rect: Rect, color: Color, radius: f32, clip: Rect) {
        if color.a == 0 || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let source = rect;
        let Some(rect) = intersect(rect, clip) else {
            return;
        };
        let x0 = rect.x.floor().max(0.0) as i32;
        let y0 = rect.y.floor().max(0.0) as i32;
        let x1 = rect.right().ceil().min(self.width as f32) as i32;
        let y1 = rect.bottom().ceil().min(self.height as f32) as i32;
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let stride = self.width as usize;
        if color.a == 255 && radius <= 0.0 {
            let packed = color.to_u32();
            for y in y0..y1 {
                let row = y as usize * stride;
                for x in x0..x1 {
                    self.pixels[row + x as usize] = packed;
                }
            }
            return;
        }
        for y in y0..y1 {
            let row = y as usize * stride;
            for x in x0..x1 {
                if radius > 0.0 && !inside_round(x, y, source, radius) {
                    continue;
                }
                let index = row + x as usize;
                if color.a == 255 {
                    self.pixels[index] = color.to_u32();
                } else {
                    self.pixels[index] = blend(self.pixels[index], color, 255);
                }
            }
        }
    }

    /// Nonzero-winding fill. Each contour is closed from its last point to its first.
    pub fn fill_contours(
        &mut self,
        contours: &[Vec<(f32, f32)>],
        color: Color,
        clip: Rect,
        even_odd: bool,
    ) {
        if color.a == 0 || contours.is_empty() {
            return;
        }
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for contour in contours {
            for (x, y) in contour {
                min_x = min_x.min(*x);
                min_y = min_y.min(*y);
                max_x = max_x.max(*x);
                max_y = max_y.max(*y);
            }
        }
        if max_x <= min_x || max_y <= min_y {
            return;
        }
        let bounds = Rect::new(min_x, min_y, max_x - min_x, max_y - min_y);
        let Some(rect) = intersect(bounds, clip) else {
            return;
        };
        let x0 = rect.x.floor().max(0.0) as i32;
        let y0 = rect.y.floor().max(0.0) as i32;
        let x1 = rect.right().ceil().min(self.width as f32) as i32;
        let y1 = rect.bottom().ceil().min(self.height as f32) as i32;
        let stride = self.width as usize;
        for y in y0..y1 {
            let row = y as usize * stride;
            for x in x0..x1 {
                let mut hits = 0;
                for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let wind = winding(x as f32 + ox, y as f32 + oy, contours);
                    let inside = if even_odd { wind.abs() % 2 == 1 } else { wind != 0 };
                    if inside {
                        hits += 1;
                    }
                }
                if hits == 0 {
                    continue;
                }
                let index = row + x as usize;
                let coverage = (hits * 255 / 4) as u8;
                self.pixels[index] = blend(self.pixels[index], color, coverage);
            }
        }
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color, clip: Rect) {
        if radius <= 0.0 || color.a == 0 {
            return;
        }
        self.fill_round(
            Rect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0),
            color,
            radius,
            clip,
        );
    }

    pub fn mask(&mut self, dest: Rect, bitmap: &Bitmap, tint: Color, clip: Rect) {
        if dest.width <= 0.0 || dest.height <= 0.0 || bitmap.width == 0 || bitmap.height == 0 {
            return;
        }
        let Some(visible) = intersect(dest, clip) else {
            return;
        };
        let x0 = visible.x.floor().max(0.0) as i32;
        let y0 = visible.y.floor().max(0.0) as i32;
        let x1 = visible.right().ceil().min(self.width as f32) as i32;
        let y1 = visible.bottom().ceil().min(self.height as f32) as i32;
        for y in y0..y1 {
            for x in x0..x1 {
                let u = ((x as f32 + 0.5 - dest.x) / dest.width * bitmap.width as f32).floor();
                let v = ((y as f32 + 0.5 - dest.y) / dest.height * bitmap.height as f32).floor();
                let u = (u as i64).clamp(0, bitmap.width as i64 - 1) as u32;
                let v = (v as i64).clamp(0, bitmap.height as i64 - 1) as u32;
                let packed = bitmap.pixels[(v * bitmap.width + u) as usize];
                let alpha = ((packed >> 24) as u32 * u32::from(tint.a) / 255) as u8;
                if alpha == 0 {
                    continue;
                }
                let color = Color::rgba(tint.r, tint.g, tint.b, alpha);
                let index = y as usize * self.width as usize + x as usize;
                self.pixels[index] = if alpha == 255 {
                    color.to_u32()
                } else {
                    blend(self.pixels[index], color, 255)
                };
            }
        }
    }

    pub fn image(&mut self, dest: Rect, bitmap: &Bitmap, clip: Rect) {
        if dest.width <= 0.0 || dest.height <= 0.0 || bitmap.width == 0 || bitmap.height == 0 {
            return;
        }
        let Some(visible) = intersect(dest, clip) else {
            return;
        };
        let x0 = visible.x.floor().max(0.0) as i32;
        let y0 = visible.y.floor().max(0.0) as i32;
        let x1 = visible.right().ceil().min(self.width as f32) as i32;
        let y1 = visible.bottom().ceil().min(self.height as f32) as i32;
        for y in y0..y1 {
            for x in x0..x1 {
                let u = ((x as f32 + 0.5 - dest.x) / dest.width * bitmap.width as f32).floor();
                let v = ((y as f32 + 0.5 - dest.y) / dest.height * bitmap.height as f32).floor();
                let u = (u as i64).clamp(0, bitmap.width as i64 - 1) as u32;
                let v = (v as i64).clamp(0, bitmap.height as i64 - 1) as u32;
                let packed = bitmap.pixels[(v * bitmap.width + u) as usize];
                let color = Color::rgba(
                    (packed >> 16) as u8,
                    (packed >> 8) as u8,
                    packed as u8,
                    (packed >> 24) as u8,
                );
                if color.a == 0 {
                    continue;
                }
                let index = y as usize * self.width as usize + x as usize;
                self.pixels[index] = if color.a == 255 {
                    color.to_u32()
                } else {
                    blend(self.pixels[index], color, 255)
                };
            }
        }
    }

    pub fn text(
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
        self.text_family(
            fonts, x, baseline, text, size, color, bold, mono, underline, "", clip,
        );
    }

    pub fn text_family(
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
        if text.is_empty() || color.a == 0 || size <= 0.0 {
            return;
        }
        self.draw_glyphs(
            fonts, x, baseline, text, size, color, bold, mono, underline, family, None, clip,
        );
    }

    /// Draw `text` in the layout slot `slot_width`. A wider shaping is compressed
    /// so it stays inside the width Parley already measured.
    pub fn text_fitted(
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
        self.draw_glyphs(
            fonts,
            x,
            baseline,
            text,
            size,
            color,
            bold,
            mono,
            underline,
            family,
            Some(slot_width),
            clip,
        );
    }

    fn draw_glyphs(
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
        slot_width: Option<f32>,
        clip: Rect,
    ) {
        if text.is_empty() || color.a == 0 || size <= 0.0 {
            return;
        }
        let glyphs = fonts.shape(text, size, bold, mono, family);
        let shaped: f32 = glyphs.iter().map(|glyph| glyph.advance).sum();
        let fit = match slot_width {
            Some(slot) if shaped > slot + 0.5 && slot > 0.0 => slot / shaped,
            _ => 1.0,
        };
        let mut pen = x;
        for glyph in glyphs {
            if !pen.is_finite() || pen < -100_000.0 || pen > 100_000.0 {
                break;
            }
            let left = (pen.round() as i32)
                .saturating_add((glyph.xmin as f32 * fit).round() as i32)
                .saturating_add((glyph.x_offset * fit).round() as i32);
            let top = glyph_top(baseline - glyph.y_offset, &glyph);
            for row in 0..glyph.height {
                for col in 0..glyph.width {
                    let coverage = glyph.coverage[row * glyph.width + col];
                    if coverage == 0 {
                        continue;
                    }
                    let px = left + col as i32;
                    let py = top + row as i32;
                    if px < 0 || py < 0 || px >= self.width || py >= self.height {
                        continue;
                    }
                    if !clip.contains(px as f32 + 0.5, py as f32 + 0.5) {
                        continue;
                    }
                    let index = py as usize * self.width as usize + px as usize;
                    self.pixels[index] = blend(self.pixels[index], color, coverage);
                }
            }
            pen += glyph.advance * fit;
        }
        if underline {
            let width = match slot_width {
                Some(slot) if slot > 0.0 => slot,
                _ => fonts.width_family(text, size, bold, mono, family),
            };
            let y = baseline + (size * 0.12).max(1.0);
            self.fill(Rect::new(x, y, width, (size * 0.06).max(1.0)), color, clip);
        }
    }
}

fn stroke_polyline(
    canvas: &mut dyn Draw,
    points: &[(f32, f32)],
    width: f32,
    color: Color,
    clip: Rect,
) {
    if points.len() < 2 || color.a == 0 {
        return;
    }
    let radius = (width * 0.5).max(0.6);
    for pair in points.windows(2) {
        let (x0, y0) = pair[0];
        let (x1, y1) = pair[1];
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len = (dx * dx + dy * dy).sqrt();
        let steps = (len / radius.max(0.5)).ceil().max(1.0) as i32;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            canvas.fill_circle(x0 + dx * t, y0 + dy * t, radius, color, clip);
        }
    }
}

pub use vello_scene::{VelloTarget, render_vello_cpu};

/// Draw commands shared by the software rasterizer and the Vello backends.
pub trait Draw {
    fn width(&self) -> i32;
    fn height(&self) -> i32;
    fn fill(&mut self, rect: Rect, color: Color, clip: Rect);
    fn fill_vertical(&mut self, rect: Rect, top: Color, bottom: Color, clip: Rect);
    fn fill_round(&mut self, rect: Rect, color: Color, radius: f32, clip: Rect);
    fn fill_contours(
        &mut self,
        contours: &[Vec<(f32, f32)>],
        color: Color,
        clip: Rect,
        even_odd: bool,
    );
    fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color, clip: Rect);
    fn mask(&mut self, dest: Rect, bitmap: &Bitmap, tint: Color, clip: Rect);
    fn image(&mut self, dest: Rect, bitmap: &Bitmap, clip: Rect);
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
    );
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
    );
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
    );
}

impl Draw for Canvas<'_> {
    fn width(&self) -> i32 {
        self.width
    }
    fn height(&self) -> i32 {
        self.height
    }
    fn fill(&mut self, rect: Rect, color: Color, clip: Rect) {
        Canvas::fill(self, rect, color, clip);
    }
    fn fill_vertical(&mut self, rect: Rect, top: Color, bottom: Color, clip: Rect) {
        Canvas::fill_vertical(self, rect, top, bottom, clip);
    }
    fn fill_round(&mut self, rect: Rect, color: Color, radius: f32, clip: Rect) {
        Canvas::fill_round(self, rect, color, radius, clip);
    }
    fn fill_contours(
        &mut self,
        contours: &[Vec<(f32, f32)>],
        color: Color,
        clip: Rect,
        even_odd: bool,
    ) {
        Canvas::fill_contours(self, contours, color, clip, even_odd);
    }
    fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color, clip: Rect) {
        Canvas::fill_circle(self, cx, cy, radius, color, clip);
    }
    fn mask(&mut self, dest: Rect, bitmap: &Bitmap, tint: Color, clip: Rect) {
        Canvas::mask(self, dest, bitmap, tint, clip);
    }
    fn image(&mut self, dest: Rect, bitmap: &Bitmap, clip: Rect) {
        Canvas::image(self, dest, bitmap, clip);
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
        Canvas::text(
            self, fonts, x, baseline, text, size, color, bold, mono, underline, clip,
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
        Canvas::text_family(
            self, fonts, x, baseline, text, size, color, bold, mono, underline, family, clip,
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
        Canvas::text_fitted(
            self, fonts, x, baseline, text, size, color, bold, mono, underline, family,
            slot_width, clip,
        );
    }
}

pub fn paint_layout(
    canvas: &mut dyn Draw,
    tree: &LayoutTree,
    fonts: &FontBook,
    clip: Rect,
    scroll: f32,
    origin_y: f32,
) {
    paint_box(canvas, &tree.root, fonts, clip, scroll, origin_y);
}

fn paint_box(
    canvas: &mut dyn Draw,
    box_: &LayoutBox,
    fonts: &FontBook,
    clip: Rect,
    scroll: f32,
    origin_y: f32,
) {
    paint_box_with_opacity(
        canvas,
        box_,
        fonts,
        clip,
        scroll,
        origin_y,
        1.0,
        false,
        0.0,
        f32::INFINITY,
    );
}

fn paint_box_with_opacity(
    canvas: &mut dyn Draw,
    box_: &LayoutBox,
    fonts: &FontBook,
    clip: Rect,
    scroll: f32,
    origin_y: f32,
    opacity: f32,
    pinned: bool,
    shift: f32,
    parent_bottom: f32,
) {
    if !box_.visible {
        return;
    }
    let pinned = pinned || box_.fixed;
    let opacity = (opacity * box_.opacity).clamp(0.0, 1.0);
    let natural_y = if pinned {
        box_.rect.y + origin_y
    } else {
        box_.rect.y - scroll + origin_y
    };
    let mut stick = 0.0;
    if box_.sticky && !pinned {
        if let Some(top) = box_.sticky_top {
            stick = (origin_y + top - natural_y).max(0.0);
        }
        if let Some(bottom) = box_.sticky_bottom {
            let overflow = natural_y + box_.rect.height - (clip.bottom() - bottom);
            if overflow > 0.0 {
                stick = stick.min(-overflow);
            }
        }
        if parent_bottom.is_finite() {
            let room = parent_bottom - (natural_y + box_.rect.height);
            stick = stick.min(room.max(0.0));
        }
    }
    let shift = shift + stick;
    let screen_y = natural_y + shift;
    let screen = Rect::new(box_.rect.x, screen_y, box_.rect.width, box_.rect.height);
    if let Some(shadow) = box_.shadow {
        paint_shadow(canvas, screen, shadow, clip);
    }
    let background = scale_alpha(box_.background, opacity);
    if box_.mask {
        // The mask bitmap is painted below, tinted with this color.
    } else if let Some((top, bottom)) = box_.gradient {
        let top = scale_alpha(top, opacity);
        let bottom = scale_alpha(bottom, opacity);
        if box_.border_radius > 0.0 {
            canvas.fill_round(screen, mix(top, bottom), box_.border_radius, clip);
        } else {
            canvas.fill_vertical(screen, top, bottom, clip);
        }
    } else if background.a > 0 {
        canvas.fill_round(screen, background, box_.border_radius, clip);
    }
    let shape_clip = intersect(clip, screen).unwrap_or(clip);
    for shape in &box_.shapes {
        let moved: Vec<(f32, f32)> = shape
            .points
            .iter()
            .map(|(x, y)| (screen.x + x, screen.y + y))
            .collect();
        if let Some(fill) = shape.fill {
            let contours = if shape.contours.is_empty() {
                vec![moved.clone()]
            } else {
                shape
                    .contours
                    .iter()
                    .map(|contour| {
                        contour
                            .iter()
                            .map(|(x, y)| (screen.x + x, screen.y + y))
                            .collect()
                    })
                    .collect()
            };
            canvas.fill_contours(
                &contours,
                scale_alpha(fill, opacity),
                shape_clip,
                shape.even_odd,
            );
        }
        if let Some(stroke) = shape.stroke {
            let stroked = if shape.contours.is_empty() {
                vec![moved]
            } else {
                shape
                    .contours
                    .iter()
                    .map(|contour| {
                        contour
                            .iter()
                            .map(|(x, y)| (screen.x + x, screen.y + y))
                            .collect::<Vec<_>>()
                    })
                    .collect()
            };
            for contour in &stroked {
                stroke_polyline(
                    canvas,
                    contour,
                    shape.stroke_width,
                    scale_alpha(stroke, opacity),
                    shape_clip,
                );
            }
        }
    }
    if let Some(bitmap) = &box_.image {
        let left = box_.borders[3].max(0.0);
        let right = box_.borders[1].max(0.0);
        let top = box_.borders[0].max(0.0);
        let bottom = box_.borders[2].max(0.0);
        let dest = Rect::new(
            screen.x + left,
            screen.y + top,
            (screen.width - left - right).max(0.0),
            (screen.height - top - bottom).max(0.0),
        );
        if box_.mask {
            let tint = if background.a > 0 {
                background
            } else {
                scale_alpha(box_.color, opacity)
            };
            canvas.mask(dest, bitmap, tint, clip);
        } else {
            canvas.image(dest, bitmap, clip);
        }
    }
    let border_color = scale_alpha(box_.border_color, opacity);
    if border_color.a > 0 {
        let top = box_.borders[0].max(0.0);
        let right = box_.borders[1].max(0.0);
        let bottom = box_.borders[2].max(0.0);
        let left = box_.borders[3].max(0.0);
        if top > 0.0 {
            canvas.fill(
                Rect::new(screen.x, screen.y, screen.width, top),
                border_color,
                clip,
            );
        }
        if bottom > 0.0 {
            canvas.fill(
                Rect::new(screen.x, screen.bottom() - bottom, screen.width, bottom),
                border_color,
                clip,
            );
        }
        if left > 0.0 {
            canvas.fill(
                Rect::new(screen.x, screen.y, left, screen.height),
                border_color,
                clip,
            );
        }
        if right > 0.0 {
            canvas.fill(
                Rect::new(screen.right() - right, screen.y, right, screen.height),
                border_color,
                clip,
            );
        }
    }
    let child_clip = if box_.overflow_hidden {
        match intersect(clip, screen) {
            Some(next) => next,
            None => return,
        }
    } else {
        clip
    };
    if let Some(marker) = &box_.marker {
        let baseline = box_
            .lines
            .first()
            .map(|line| {
                if pinned {
                    line.baseline + origin_y
                } else {
                    line.baseline - scroll + origin_y
                }
            })
            .unwrap_or(screen.y + box_.font_size)
            + shift;
        canvas.text(
            fonts,
            screen.x - 18.0,
            baseline,
            marker,
            box_.font_size,
            scale_alpha(box_.color, opacity),
            box_.bold,
            box_.mono,
            false,
            child_clip,
        );
    }
    for line in &box_.lines {
        let baseline = if pinned {
            line.baseline + origin_y
        } else {
            line.baseline - scroll + origin_y
        } + shift;
        for fragment in &line.fragments {
            canvas.text_fitted(
                fonts,
                fragment.x,
                baseline,
                &fragment.text,
                fragment.size,
                scale_alpha(fragment.color, opacity),
                fragment.bold,
                fragment.mono,
                fragment.underline,
                &fragment.family,
                fragment.width,
                child_clip,
            );
        }
    }
    let mut order: Vec<usize> = (0..box_.children.len()).collect();
    order.sort_by_key(|index| box_.children[*index].z_index);
    for index in order {
        paint_box_with_opacity(
            canvas,
            &box_.children[index],
            fonts,
            child_clip,
            scroll,
            origin_y,
            opacity,
            pinned,
            shift,
            screen.bottom(),
        );
    }
    if let Some(frame) = &box_.frame {
        let mut nested = frame.root.clone();
        let dx = screen.x - nested.rect.x;
        let dy = screen.y - nested.rect.y;
        shift_box(&mut nested, dx, dy);
        let frame_clip = intersect(child_clip, screen).unwrap_or(screen);
        paint_box_with_opacity(
            canvas,
            &nested,
            fonts,
            frame_clip,
            0.0,
            0.0,
            opacity,
            true,
            0.0,
            f32::INFINITY,
        );
    }
}

fn shift_box(box_: &mut LayoutBox, dx: f32, dy: f32) {
    box_.rect.x += dx;
    box_.rect.y += dy;
    for line in &mut box_.lines {
        line.baseline += dy;
        for fragment in &mut line.fragments {
            fragment.x += dx;
        }
    }
    for child in &mut box_.children {
        shift_box(child, dx, dy);
    }
}

fn paint_shadow(canvas: &mut dyn Draw, screen: Rect, shadow: mithu_style::BoxShadow, clip: Rect) {
    let layers = 4;
    for layer in 1..=layers {
        let t = layer as f32 / layers as f32;
        let grow = shadow.blur.max(0.0) * t + shadow.spread;
        let alpha = (f32::from(shadow.color.a) * (1.0 - t) * 0.45) as u8;
        if alpha == 0 {
            continue;
        }
        let color = mithu_base::Color::rgba(shadow.color.r, shadow.color.g, shadow.color.b, alpha);
        let rect = Rect::new(
            screen.x + shadow.x - grow,
            screen.y + shadow.y - grow,
            (screen.width + grow * 2.0).max(0.0),
            (screen.height + grow * 2.0).max(0.0),
        );
        canvas.fill_round(rect, color, screen.height.min(8.0) + grow, clip);
    }
}

fn scale_alpha(color: Color, opacity: f32) -> Color {
    Color {
        r: color.r,
        g: color.g,
        b: color.b,
        a: (f32::from(color.a) * opacity).round().clamp(0.0, 255.0) as u8,
    }
}

fn mix(top: Color, bottom: Color) -> Color {
    mix_t(top, bottom, 0.5)
}

fn mix_t(top: Color, bottom: Color, t: f32) -> Color {
    let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color {
        r: lerp(top.r, bottom.r),
        g: lerp(top.g, bottom.g),
        b: lerp(top.b, bottom.b),
        a: lerp(top.a, bottom.a),
    }
}

fn inside_round(x: i32, y: i32, rect: Rect, radius: f32) -> bool {
    let radius = radius.min(rect.width / 2.0).min(rect.height / 2.0);
    if radius <= 0.0 {
        return true;
    }
    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;
    let inner_left = rect.x + radius;
    let inner_right = rect.right() - radius;
    let inner_top = rect.y + radius;
    let inner_bottom = rect.bottom() - radius;
    let cx = if px < inner_left {
        inner_left
    } else if px > inner_right {
        inner_right
    } else {
        return true;
    };
    let cy = if py < inner_top {
        inner_top
    } else if py > inner_bottom {
        inner_bottom
    } else {
        return true;
    };
    let dx = px - cx;
    let dy = py - cy;
    dx * dx + dy * dy <= radius * radius
}

fn winding(px: f32, py: f32, contours: &[Vec<(f32, f32)>]) -> i32 {
    let mut wind = 0;
    for contour in contours {
        let count = contour.len();
        if count < 2 {
            continue;
        }
        for index in 0..count {
            let (x0, y0) = contour[index];
            let (x1, y1) = contour[(index + 1) % count];
            if (y0 <= py && y1 > py) || (y1 <= py && y0 > py) {
                let t = (py - y0) / (y1 - y0);
                if x0 + t * (x1 - x0) > px {
                    wind += if y1 > y0 { 1 } else { -1 };
                }
            }
        }
    }
    wind
}

pub(crate) fn intersect(rect: Rect, clip: Rect) -> Option<Rect> {
    let x0 = rect.x.max(clip.x);
    let y0 = rect.y.max(clip.y);
    let x1 = rect.right().min(clip.right());
    let y1 = rect.bottom().min(clip.bottom());
    if x1 > x0 && y1 > y0 {
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    } else {
        None
    }
}

fn blend(dst: u32, color: Color, coverage: u8) -> u32 {
    if coverage == 0 || color.a == 0 {
        return dst;
    }
    let alpha = (u32::from(coverage) * u32::from(color.a)) / 255;
    if alpha == 0 {
        return dst;
    }
    if alpha >= 255 {
        return color.to_u32();
    }
    let inverse = 255 - alpha;
    let dr = (dst >> 16) & 255;
    let dg = (dst >> 8) & 255;
    let db = dst & 255;
    let r = (u32::from(color.r) * alpha + dr * inverse) / 255;
    let g = (u32::from(color.g) * alpha + dg * inverse) / 255;
    let b = (u32::from(color.b) * alpha + db * inverse) / 255;
    (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mask_keeps_clear_pixels_unpainted() {
        let mut pixels = vec![Color::WHITE.to_u32(); 4 * 2];
        let mut canvas = Canvas {
            pixels: &mut pixels,
            width: 4,
            height: 2,
        };
        let bitmap = mithu_image::Bitmap {
            width: 2,
            height: 1,
            pixels: vec![0xFF00_0000, 0x0000_0000],
        };
        canvas.mask(
            Rect::new(0.0, 0.0, 4.0, 2.0),
            &bitmap,
            Color::rgb(32, 33, 34),
            Rect::new(0.0, 0.0, 4.0, 2.0),
        );
        assert_eq!(pixels[0], Color::rgb(32, 33, 34).to_u32());
        assert_eq!(pixels[3], Color::WHITE.to_u32());
    }

    #[test]
    fn glyph_paints_above_its_baseline() {
        let fonts = FontBook::load();
        let mut pixels = vec![Color::WHITE.to_u32(); 80 * 80];
        let mut canvas = Canvas {
            pixels: &mut pixels,
            width: 80,
            height: 80,
        };
        let ink = Color::rgb(200, 0, 0);
        canvas.text(
            &fonts,
            8.0,
            50.0,
            "A",
            40.0,
            ink,
            false,
            false,
            false,
            Rect::new(0.0, 0.0, 80.0, 80.0),
        );
        let above = pixels
            .iter()
            .take(50 * 80)
            .any(|pixel| *pixel != Color::WHITE.to_u32());
        assert!(above, "glyph should rise above the baseline");
    }
}
