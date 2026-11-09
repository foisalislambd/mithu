//! Text measurement and glyph rasterization.
//!
//! Vector faces come from the system font directories. Bengali falls back to
//! Noto Sans Bengali when the primary face has no glyph. A bitmap advance is
//! used only when no face can be loaded.

use std::path::Path;

use fontdue::Font;

pub trait MeasureText {
    fn width(&self, text: &str, size: f32, bold: bool, mono: bool) -> f32;
    fn line_height(&self, size: f32, mono: bool) -> f32;
    fn ascent(&self, size: f32, mono: bool) -> f32;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NaiveMetrics;

impl MeasureText for NaiveMetrics {
    fn width(&self, text: &str, size: f32, _bold: bool, mono: bool) -> f32 {
        let factor = if mono { 0.6 } else { 0.5 };
        text.chars().count() as f32 * size * factor
    }

    fn line_height(&self, size: f32, _mono: bool) -> f32 {
        size * 1.25
    }

    fn ascent(&self, size: f32, _mono: bool) -> f32 {
        size * 0.8
    }
}

pub struct Glyph {
    pub xmin: i32,
    pub ymin: i32,
    pub width: usize,
    pub height: usize,
    pub advance: f32,
    pub coverage: Vec<u8>,
}

enum Face {
    Vector(Font),
    Fallback,
}

pub struct FontBook {
    sans: Face,
    bold: Face,
    mono: Face,
    bengali: Face,
}

impl FontBook {
    pub fn load() -> Self {
        Self {
            sans: load_face(&[
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
                "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            ]),
            bold: load_face(&[
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
                "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
            ]),
            mono: load_face(&[
                "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
                "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
            ]),
            bengali: load_face(&["/usr/share/fonts/truetype/noto/NotoSansBengali-Regular.ttf"]),
        }
    }

    pub fn glyph(&self, ch: char, size: f32, bold: bool, mono: bool) -> Glyph {
        if ch == ' ' || ch == '\u{00A0}' {
            return Glyph {
                xmin: 0,
                ymin: 0,
                width: 0,
                height: 0,
                advance: self.advance(ch, size, bold, mono),
                coverage: Vec::new(),
            };
        }
        match self.face_for(ch, bold, mono) {
            Face::Vector(font) => {
                let (metrics, coverage) = font.rasterize(ch, size);
                Glyph {
                    xmin: metrics.xmin,
                    ymin: metrics.ymin,
                    width: metrics.width,
                    height: metrics.height,
                    advance: metrics.advance_width,
                    coverage,
                }
            }
            Face::Fallback => Glyph {
                xmin: 0,
                ymin: -(size * 0.75) as i32,
                width: (size * 0.45).max(1.0) as usize,
                height: (size * 0.75).max(1.0) as usize,
                advance: size * 0.5,
                coverage: vec![255; ((size * 0.45).max(1.0) as usize) * ((size * 0.75).max(1.0) as usize)],
            },
        }
    }

    fn advance(&self, ch: char, size: f32, bold: bool, mono: bool) -> f32 {
        match self.face_for(ch, bold, mono) {
            Face::Vector(font) => font.metrics(ch, size).advance_width,
            Face::Fallback => size * if mono { 0.6 } else { 0.5 },
        }
    }

    fn face_for(&self, ch: char, bold: bool, mono: bool) -> &Face {
        let primary = if mono { &self.mono } else if bold { &self.bold } else { &self.sans };
        if covers(primary, ch) {
            return primary;
        }
        if covers(&self.bengali, ch) {
            return &self.bengali;
        }
        if covers(&self.sans, ch) {
            return &self.sans;
        }
        primary
    }

    fn metrics_face(&self, mono: bool) -> &Face {
        let face = if mono { &self.mono } else { &self.sans };
        if matches!(face, Face::Fallback) { &self.sans } else { face }
    }
}

impl MeasureText for FontBook {
    fn width(&self, text: &str, size: f32, bold: bool, mono: bool) -> f32 {
        text.chars().map(|ch| self.advance(ch, size, bold, mono)).sum()
    }

    fn line_height(&self, size: f32, mono: bool) -> f32 {
        match self.metrics_face(mono) {
            Face::Vector(font) => font
                .horizontal_line_metrics(size)
                .map(|metrics| metrics.new_line_size)
                .unwrap_or(size * 1.25),
            Face::Fallback => size * 1.25,
        }
    }

    fn ascent(&self, size: f32, mono: bool) -> f32 {
        match self.metrics_face(mono) {
            Face::Vector(font) => font
                .horizontal_line_metrics(size)
                .map(|metrics| metrics.ascent)
                .unwrap_or(size * 0.8),
            Face::Fallback => size * 0.8,
        }
    }
}

fn load_face(paths: &[&str]) -> Face {
    for path in paths {
        if !Path::new(path).is_file() {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if let Ok(font) = Font::from_bytes(bytes, fontdue::FontSettings::default()) {
            return Face::Vector(font);
        }
    }
    Face::Fallback
}

fn covers(face: &Face, ch: char) -> bool {
    match face {
        Face::Vector(font) => font.lookup_glyph_index(ch) != 0,
        Face::Fallback => false,
    }
}

/// Screen y of a glyph bitmap's top, with y growing downward.
pub fn glyph_top(baseline: f32, glyph: &Glyph) -> i32 {
    // fontdue's ymin is a y-up offset of the bitmap bottom. Screen y grows down,
    // so the bitmap top sits at baseline - (ymin + height)... except ymin for a
    // baseline-hugging glyph is already the distance above the baseline in the
    // versions we rasterize. Prefer the placement that puts ink on or above the
    // baseline when the raw formula would push the whole glyph below it.
    let baseline = baseline.round() as i32;
    let above = baseline - glyph.ymin - glyph.height as i32;
    if above < baseline {
        above
    } else {
        baseline - glyph.height as i32
    }
}
