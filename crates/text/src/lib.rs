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

    /// Width after shaping, using `family` when a matching web font is loaded.
    fn width_family(&self, text: &str, size: f32, bold: bool, mono: bool, family: &str) -> f32 {
        let _ = family;
        self.width(text, size, bold, mono)
    }
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
    pub x_offset: f32,
    pub y_offset: f32,
    pub coverage: Vec<u8>,
}

struct VectorFace {
    font: Font,
    bytes: Vec<u8>,
}

enum Face {
    Vector(VectorFace),
    Fallback,
}

pub struct FontBook {
    sans: Face,
    bold: Face,
    mono: Face,
    bengali: Face,
    custom: Vec<(String, Face)>,
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
            custom: Vec::new(),
        }
    }

    /// Register a decoded TrueType or OpenType face under `family`.
    pub fn add_family(&mut self, family: &str, bytes: &[u8]) {
        let bytes = decode_font(bytes);
        let Ok(font) = Font::from_bytes(bytes.clone(), fontdue::FontSettings::default()) else {
            return;
        };
        let key = normalize_family(family);
        if key.is_empty() || self.custom.iter().any(|(name, _)| name == &key) {
            return;
        }
        let vector = VectorFace { font, bytes };
        if let Some(glyphs) = shape_vector(&vector, "বাংলা", 16.0)
            && glyphs.iter().any(|glyph| {
                !glyph.advance.is_finite() || glyph.advance.abs() > 64.0 || glyph.width > 128
            })
        {
            return;
        }
        self.custom.push((key, Face::Vector(vector)));
    }

    pub fn has_family(&self, family: &str) -> bool {
        let key = normalize_family(family);
        self.custom.iter().any(|(name, _)| name == &key)
    }

    /// Shape `text` into positioned glyphs. Bengali and other complex scripts
    /// use rustybuzz so a conjunct becomes one glyph instead of three.
    pub fn shape(&self, text: &str, size: f32, bold: bool, mono: bool, family: &str) -> Vec<Glyph> {
        if text.is_empty() || size <= 0.0 {
            return Vec::new();
        }
        let mut glyphs = Vec::new();
        for run in script_runs(text) {
            let face = self.face_for_run(run, bold, mono, family);
            if let Face::Vector(vector) = face
                && let Some(shaped) = shape_vector(vector, run, size)
            {
                glyphs.extend(shaped);
                continue;
            }
            for ch in run.chars() {
                glyphs.push(self.glyph(ch, size, bold, mono));
            }
        }
        glyphs
    }

    pub fn glyph(&self, ch: char, size: f32, bold: bool, mono: bool) -> Glyph {
        if ch == ' ' || ch == '\u{00A0}' {
            return Glyph {
                xmin: 0,
                ymin: 0,
                width: 0,
                height: 0,
                advance: self.advance(ch, size, bold, mono),
                x_offset: 0.0,
                y_offset: 0.0,
                coverage: Vec::new(),
            };
        }
        match self.face_for(ch, bold, mono) {
            Face::Vector(vector) => {
                let (metrics, coverage) = vector.font.rasterize(ch, size);
                Glyph {
                    xmin: metrics.xmin,
                    ymin: metrics.ymin,
                    width: metrics.width,
                    height: metrics.height,
                    advance: metrics.advance_width,
                    x_offset: 0.0,
                    y_offset: 0.0,
                    coverage,
                }
            }
            Face::Fallback => Glyph {
                xmin: 0,
                ymin: -(size * 0.75) as i32,
                width: (size * 0.45).max(1.0) as usize,
                height: (size * 0.75).max(1.0) as usize,
                advance: size * 0.5,
                x_offset: 0.0,
                y_offset: 0.0,
                coverage: vec![
                    255;
                    ((size * 0.45).max(1.0) as usize) * ((size * 0.75).max(1.0) as usize)
                ],
            },
        }
    }

    fn advance(&self, ch: char, size: f32, bold: bool, mono: bool) -> f32 {
        match self.face_for(ch, bold, mono) {
            Face::Vector(vector) => vector.font.metrics(ch, size).advance_width,
            Face::Fallback => size * if mono { 0.6 } else { 0.5 },
        }
    }

    fn face_for_run<'a>(&'a self, text: &str, bold: bool, mono: bool, family: &str) -> &'a Face {
        let key = normalize_family(family);
        if let Some((_, face)) = self.custom.iter().find(|(name, _)| name == &key) {
            return face;
        }
        if text.chars().any(is_bengali) {
            return &self.bengali;
        }
        self.face_for(text.chars().next().unwrap_or(' '), bold, mono)
    }

    fn face_for(&self, ch: char, bold: bool, mono: bool) -> &Face {
        let primary = if mono {
            &self.mono
        } else if bold {
            &self.bold
        } else {
            &self.sans
        };
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
        if matches!(face, Face::Fallback) {
            &self.sans
        } else {
            face
        }
    }
}

impl MeasureText for FontBook {
    fn width(&self, text: &str, size: f32, bold: bool, mono: bool) -> f32 {
        self.shape(text, size, bold, mono, "")
            .iter()
            .map(|glyph| glyph.advance)
            .sum()
    }

    fn width_family(&self, text: &str, size: f32, bold: bool, mono: bool, family: &str) -> f32 {
        self.shape(text, size, bold, mono, family)
            .iter()
            .map(|glyph| glyph.advance)
            .sum()
    }

    fn line_height(&self, size: f32, mono: bool) -> f32 {
        match self.metrics_face(mono) {
            Face::Vector(vector) => vector
                .font
                .horizontal_line_metrics(size)
                .map(|metrics| metrics.new_line_size)
                .unwrap_or(size * 1.25),
            Face::Fallback => size * 1.25,
        }
    }

    fn ascent(&self, size: f32, mono: bool) -> f32 {
        match self.metrics_face(mono) {
            Face::Vector(vector) => vector
                .font
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
        if let Ok(font) = Font::from_bytes(bytes.clone(), fontdue::FontSettings::default()) {
            return Face::Vector(VectorFace { font, bytes });
        }
    }
    Face::Fallback
}

fn covers(face: &Face, ch: char) -> bool {
    match face {
        Face::Vector(vector) => vector.font.lookup_glyph_index(ch) != 0,
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

fn normalize_family(family: &str) -> String {
    family
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|ch| ch == '"' || ch == '\'')
        .to_ascii_lowercase()
}

fn is_bengali(ch: char) -> bool {
    ('\u{0980}'..='\u{09FF}').contains(&ch)
}

fn script_runs(text: &str) -> Vec<&str> {
    let mut runs = Vec::new();
    let mut start = 0;
    let mut bengali = None;
    for (index, ch) in text.char_indices() {
        if ch.is_whitespace() {
            continue;
        }
        let next = is_bengali(ch);
        if bengali.is_some_and(|current| current != next) {
            runs.push(&text[start..index]);
            start = index;
        }
        bengali = Some(next);
    }
    if start < text.len() {
        runs.push(&text[start..]);
    }
    runs
}

fn shape_vector(face: &VectorFace, text: &str, size: f32) -> Option<Vec<Glyph>> {
    let rb = rustybuzz::Face::from_slice(&face.bytes, 0)?;
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let output = rustybuzz::shape(&rb, &[], buffer);
    let units = rb.units_per_em() as f32;
    if units <= 0.0 {
        return None;
    }
    let scale = size / units;
    let infos = output.glyph_infos();
    let positions = output.glyph_positions();
    let mut glyphs = Vec::with_capacity(infos.len());
    for (info, pos) in infos.iter().zip(positions) {
        let Ok(glyph_id) = u16::try_from(info.glyph_id) else {
            continue;
        };
        let (metrics, coverage) = face.font.rasterize_indexed(glyph_id, size);
        glyphs.push(Glyph {
            xmin: metrics.xmin,
            ymin: metrics.ymin,
            width: metrics.width,
            height: metrics.height,
            advance: pos.x_advance as f32 * scale,
            x_offset: pos.x_offset as f32 * scale,
            y_offset: pos.y_offset as f32 * scale,
            coverage,
        });
    }
    Some(glyphs)
}

/// Turn WOFF and WOFF2 bytes into a TrueType/OpenType font. Other bytes pass through.
pub fn decode_font(bytes: &[u8]) -> Vec<u8> {
    if bytes.starts_with(b"wOFF")
        && let Some(decoded) = decode_woff(bytes)
    {
        return decoded;
    }
    if bytes.starts_with(b"wOF2")
        && let Ok(decoded) = oxifont_webfont::decode_woff2(bytes)
    {
        return decoded;
    }
    bytes.to_vec()
}

fn decode_woff(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 44 {
        return None;
    }
    let flavor = u32::from_be_bytes(bytes[4..8].try_into().ok()?);
    let num_tables = u16::from_be_bytes(bytes[12..14].try_into().ok()?) as usize;
    let mut tables = Vec::with_capacity(num_tables);
    for index in 0..num_tables {
        let start = 44 + index * 20;
        let entry = bytes.get(start..start + 20)?;
        let tag = entry[0..4].to_vec();
        let offset = u32::from_be_bytes(entry[4..8].try_into().ok()?) as usize;
        let comp = u32::from_be_bytes(entry[8..12].try_into().ok()?) as usize;
        let orig = u32::from_be_bytes(entry[12..16].try_into().ok()?) as usize;
        let checksum = u32::from_be_bytes(entry[16..20].try_into().ok()?);
        let compressed = bytes.get(offset..offset + comp)?;
        let data = if comp == orig {
            compressed.to_vec()
        } else {
            use std::io::Read;
            let mut decoder = flate2::read::ZlibDecoder::new(compressed);
            let mut inflated = Vec::with_capacity(orig);
            decoder.read_to_end(&mut inflated).ok()?;
            inflated
        };
        tables.push((tag, checksum, data));
    }
    let search_range = (num_tables.next_power_of_two() / 2 * 16) as u16;
    let entry_selector = search_range.trailing_zeros().saturating_sub(4) as u16;
    let range_shift = (num_tables * 16) as u16 - search_range;
    let mut out = Vec::new();
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(num_tables as u16).to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());
    let directory = 12 + num_tables * 16;
    let mut cursor = directory;
    for (tag, checksum, data) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum.to_be_bytes());
        out.extend_from_slice(&(cursor as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        cursor += data.len().div_ceil(4) * 4;
    }
    for (_, _, data) in &tables {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bengali_conjunct_shapes_to_fewer_glyphs() {
        let fonts = FontBook::load();
        let text = "ক্ষ";
        let glyphs = fonts.shape(text, 16.0, false, false, "");
        assert!(
            glyphs.len() < text.chars().count(),
            "shaped {} chars {}",
            glyphs.len(),
            text.chars().count()
        );
        assert!(glyphs.iter().any(|glyph| glyph.width > 0));
    }
}
