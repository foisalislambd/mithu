//! The official Mithu mark: a myna over a blue globe with an orange ring.
//!
//! `mithu-logo.png` keeps the wordmark for the new-tab page, About, and the
//! README. `mithu-mark.png` is the same bird, globe, and ring without the
//! wordmark, for sizes where the letters would blur.

use std::io::Cursor;
use std::sync::OnceLock;

pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn decode(bytes: &[u8]) -> Raster {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("mithu logo png");
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("mithu logo frame");
    let pixels = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(pixels.len() / 3 * 4);
            for chunk in pixels.chunks_exact(3) {
                out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            out
        }
        _ => panic!("mithu logo must be rgb or rgba"),
    };
    Raster { width: info.width, height: info.height, rgba }
}

pub fn mark() -> &'static Raster {
    static MARK: OnceLock<Raster> = OnceLock::new();
    MARK.get_or_init(|| decode(include_bytes!("../../../assets/brand/png/mark-256.png")))
}

pub fn logo() -> &'static Raster {
    static LOGO: OnceLock<Raster> = OnceLock::new();
    LOGO.get_or_init(|| decode(include_bytes!("../../../assets/brand/png/logo-512.png")))
}

pub fn window_icon(lite: bool) -> Raster {
    if lite {
        decode(include_bytes!("../../../assets/brand/png/mark-lite-128.png"))
    } else {
        decode(include_bytes!("../../../assets/brand/png/mark-128.png"))
    }
}

pub fn favicon_data_uri() -> String {
    data_uri(include_bytes!("../../../assets/brand/png/mark-32.png"))
}

pub fn mark_data_uri() -> String {
    data_uri(include_bytes!("../../../assets/brand/png/mark-64.png"))
}

pub fn logo_data_uri() -> String {
    data_uri(include_bytes!("../../../assets/brand/png/logo-512.png"))
}

fn data_uri(png: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::from("data:image/png;base64,");
    let mut i = 0;
    while i + 3 <= png.len() {
        let n = ((png[i] as u32) << 16) | ((png[i + 1] as u32) << 8) | png[i + 2] as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        i += 3;
    }
    if i < png.len() {
        let a = png[i];
        let b = if i + 1 < png.len() { png[i + 1] } else { 0 };
        let n = ((a as u32) << 16) | ((b as u32) << 8);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if i + 1 < png.len() {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
            out.push('=');
        } else {
            out.push('=');
            out.push('=');
        }
    }
    out
}

pub fn blit(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, dw: f32, dh: f32, image: &Raster) {
    let dw = dw.round().max(1.0) as i32;
    let dh = dh.round().max(1.0) as i32;
    let x0 = x.round() as i32;
    let y0 = y.round() as i32;
    for row in 0..dh {
        for col in 0..dw {
            let sx = ((col as f32 + 0.5) * image.width as f32 / dw as f32 - 0.5).clamp(0.0, image.width as f32 - 1.0);
            let sy = ((row as f32 + 0.5) * image.height as f32 / dh as f32 - 0.5).clamp(0.0, image.height as f32 - 1.0);
            let (r, g, b, a) = sample(image, sx, sy);
            if a == 0 {
                continue;
            }
            blend(pixels, width, height, x0 + col, y0 + row, (r << 16) | (g << 8) | b, a);
        }
    }
}

fn sample(image: &Raster, x: f32, y: f32) -> (u32, u32, u32, u8) {
    let x1 = x.floor() as u32;
    let y1 = y.floor() as u32;
    let x2 = (x1 + 1).min(image.width - 1);
    let y2 = (y1 + 1).min(image.height - 1);
    let tx = x - x1 as f32;
    let ty = y - y1 as f32;
    let p00 = pixel(image, x1, y1);
    let p10 = pixel(image, x2, y1);
    let p01 = pixel(image, x1, y2);
    let p11 = pixel(image, x2, y2);
    let mix = |a: u8, b: u8, t: f32| a as f32 + (b as f32 - a as f32) * t;
    let channel = |index: usize| {
        let top = mix(p00[index], p10[index], tx);
        let bottom = mix(p01[index], p11[index], tx);
        (top + (bottom - top) * ty).round() as u32
    };
    (channel(0), channel(1), channel(2), channel(3) as u8)
}

fn pixel(image: &Raster, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * image.width + x) * 4) as usize;
    [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2], image.rgba[i + 3]]
}

fn blend(pixels: &mut [u32], width: u32, height: u32, x: i32, y: i32, color: u32, alpha: u8) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let dest = &mut pixels[(y as u32 * width + x as u32) as usize];
    let a = alpha as u32;
    let inv = 255 - a;
    let sr = (color >> 16) & 255;
    let sg = (color >> 8) & 255;
    let sb = color & 255;
    let dr = (*dest >> 16) & 255;
    let dg = (*dest >> 8) & 255;
    let db = *dest & 255;
    *dest = (((sr * a + dr * inv) / 255) << 16) | (((sg * a + dg * inv) / 255) << 8) | ((sb * a + db * inv) / 255);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_corners_are_clear_and_the_bird_is_not() {
        let mark = mark();
        assert_eq!((mark.width, mark.height), (256, 256));
        assert_eq!(mark.rgba[3], 0, "top-left pixel should be transparent");
        let mid = ((128 * 256 + 128) * 4 + 3) as usize;
        assert!(mark.rgba[mid] > 200, "the center of the mark should be opaque");
    }
}
