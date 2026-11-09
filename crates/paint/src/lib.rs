//! Software rasterizer. Pixels are packed `0x00RRGGBB`.

use mithu_base::{Color, Rect};
use mithu_layout::{LayoutBox, LayoutTree};
use mithu_text::{glyph_top, FontBook, MeasureText};

pub struct Canvas<'a> {
    pub pixels: &'a mut [u32],
    pub width: i32,
    pub height: i32,
}

impl Canvas<'_> {
    pub fn fill(&mut self, rect: Rect, color: Color, clip: Rect) {
        self.fill_round(rect, color, 0.0, clip);
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
        if text.is_empty() || color.a == 0 || size <= 0.0 {
            return;
        }
        let mut pen = x;
        for ch in text.chars() {
            let glyph = fonts.glyph(ch, size, bold, mono);
            let left = pen.round() as i32 + glyph.xmin;
            let top = glyph_top(baseline, &glyph);
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
            pen += glyph.advance;
        }
        if underline {
            let width = fonts.width(text, size, bold, mono);
            let y = baseline + (size * 0.12).max(1.0);
            self.fill(Rect::new(x, y, width, (size * 0.06).max(1.0)), color, clip);
        }
    }
}

pub fn paint_layout(
    canvas: &mut Canvas<'_>,
    tree: &LayoutTree,
    fonts: &FontBook,
    clip: Rect,
    scroll: f32,
    origin_y: f32,
) {
    paint_box(canvas, &tree.root, fonts, clip, scroll, origin_y);
}

fn paint_box(canvas: &mut Canvas<'_>, box_: &LayoutBox, fonts: &FontBook, clip: Rect, scroll: f32, origin_y: f32) {
    paint_box_with_opacity(canvas, box_, fonts, clip, scroll, origin_y, 1.0);
}

fn paint_box_with_opacity(
    canvas: &mut Canvas<'_>,
    box_: &LayoutBox,
    fonts: &FontBook,
    clip: Rect,
    scroll: f32,
    origin_y: f32,
    opacity: f32,
) {
    let opacity = (opacity * box_.opacity).clamp(0.0, 1.0);
    let screen = Rect::new(
        box_.rect.x,
        box_.rect.y - scroll + origin_y,
        box_.rect.width,
        box_.rect.height,
    );
    let background = scale_alpha(box_.background, opacity);
    if background.a > 0 {
        canvas.fill_round(screen, background, box_.border_radius, clip);
    }
    let border = box_.border_width;
    let border_color = scale_alpha(box_.border_color, opacity);
    if border > 0.0 && border_color.a > 0 {
        canvas.fill(Rect::new(screen.x, screen.y, screen.width, border), border_color, clip);
        canvas.fill(
            Rect::new(screen.x, screen.bottom() - border, screen.width, border),
            border_color,
            clip,
        );
        canvas.fill(Rect::new(screen.x, screen.y, border, screen.height), border_color, clip);
        canvas.fill(
            Rect::new(screen.right() - border, screen.y, border, screen.height),
            border_color,
            clip,
        );
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
            .map(|line| line.baseline - scroll + origin_y)
            .unwrap_or(screen.y + box_.font_size);
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
        let baseline = line.baseline - scroll + origin_y;
        for fragment in &line.fragments {
            canvas.text(
                fonts,
                fragment.x,
                baseline,
                &fragment.text,
                fragment.size,
                scale_alpha(fragment.color, opacity),
                fragment.bold,
                fragment.mono,
                fragment.underline,
                child_clip,
            );
        }
    }
    for child in &box_.children {
        paint_box_with_opacity(canvas, child, fonts, child_clip, scroll, origin_y, opacity);
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

fn intersect(rect: Rect, clip: Rect) -> Option<Rect> {
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
        let above = pixels.iter().take(50 * 80).any(|pixel| *pixel != Color::WHITE.to_u32());
        assert!(above, "glyph should rise above the baseline");
    }
}
