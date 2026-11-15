//! Decode PNG, JPEG, GIF, WebP, and SVG into pixels the painter can blend.
//!
//! Format decoders live in the `image` crate. Rewriting JPEG would not make
//! the browser stronger. Pixels are packed `0xAARRGGBB`.

use base64::Engine;
use image::DynamicImage;

#[derive(Clone, Debug)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

/// Encode an `RGB` buffer as PNG. Used for headless screenshots. The same `image` crate that decodes page images writes the file, so the browser does not take on another dependency.
pub fn encode_png(width: u32, height: u32, rgb: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgb.len() != width as usize * height as usize * 3 {
        return None;
    }
    let image = image::RgbImage::from_raw(width, height, rgb.to_vec())?;
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(image)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .ok()?;
    Some(bytes)
}

pub fn decode(bytes: &[u8]) -> Option<Bitmap> {
    let image = image::load_from_memory(bytes).ok()?;
    from_dynamic(image)
}

/// Rasterize an SVG document with resvg. Gradients, clip paths, and masks
/// are included. The longest side is kept between the file's own size and 512px.
pub fn decode_svg(bytes: &[u8]) -> Option<Bitmap> {
    if bytes.is_empty() || bytes.len() > 2_000_000 {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    if !text.contains("<svg") && !text.contains("<SVG") {
        return None;
    }
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &options).ok()?;
    let size = tree.size();
    let long = size.width().max(size.height()).max(1.0);
    let scale = (256.0 / long).clamp(1.0, 4.0);
    let width = (size.width() * scale).round().clamp(1.0, 512.0) as u32;
    let height = (size.height() * scale).round().clamp(1.0, 512.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let pixels: Vec<u32> = pixmap
        .data()
        .chunks_exact(4)
        .map(|px| unpremultiply(px[0], px[1], px[2], px[3]))
        .collect();
    if pixels.iter().all(|pixel| *pixel == 0) {
        return None;
    }
    Some(Bitmap {
        width,
        height,
        pixels,
    })
}

fn unpremultiply(r: u8, g: u8, b: u8, a: u8) -> u32 {
    if a == 0 {
        return 0;
    }
    let (r, g, b) = if a == 255 {
        (r, g, b)
    } else {
        let scale = 255u32 / u32::from(a);
        (
            (u32::from(r) * scale).min(255) as u8,
            (u32::from(g) * scale).min(255) as u8,
            (u32::from(b) * scale).min(255) as u8,
        )
    };
    (u32::from(a) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

pub fn decode_data_url(url: &str) -> Option<Bitmap> {
    let rest = url.trim().strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let bytes = if meta.to_ascii_lowercase().contains(";base64") {
        base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .ok()?
    } else {
        percent_decode_bytes(data)
    };
    decode(&bytes)
}

fn from_dynamic(image: DynamicImage) -> Option<Bitmap> {
    let rgba = image.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    if width == 0 || height == 0 {
        return None;
    }
    if width as u64 * height as u64 > 8_000_000 {
        return None;
    }
    let pixels = rgba
        .pixels()
        .map(|pixel| {
            (u32::from(pixel[3]) << 24)
                | (u32::from(pixel[0]) << 16)
                | (u32::from(pixel[1]) << 8)
                | u32::from(pixel[2])
        })
        .collect();
    Some(Bitmap {
        width,
        height,
        pixels,
    })
}

fn percent_decode_bytes(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn decodes_a_png_and_a_data_url() {
        let mut image = RgbaImage::new(2, 1);
        image.put_pixel(0, 0, Rgba([10, 20, 30, 255]));
        image.put_pixel(1, 0, Rgba([1, 2, 3, 128]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("encode");
        let bitmap = decode(&bytes).expect("png");
        assert_eq!(bitmap.width, 2);
        assert_eq!(bitmap.height, 1);
        assert_eq!(bitmap.pixels[0], 0xFF0A141E);
        assert_eq!(bitmap.pixels[1], 0x80010203);

        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let url = format!("data:image/png;base64,{encoded}");
        let again = decode_data_url(&url).expect("data url");
        assert_eq!(again.pixels, bitmap.pixels);
    }

    #[test]
    fn rasterizes_a_gradient_clipped_to_a_circle() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40">
            <defs>
                <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
                    <stop offset="0" stop-color="#ff0000"/>
                    <stop offset="1" stop-color="#0000ff"/>
                </linearGradient>
                <clipPath id="c"><circle cx="20" cy="20" r="16"/></clipPath>
            </defs>
            <rect width="40" height="40" fill="url(#g)" clip-path="url(#c)"/>
        </svg>"##;
        let bitmap = decode_svg(svg).expect("svg");
        assert!(bitmap.width >= 40 && bitmap.height >= 40);
        let center = bitmap.pixels[(bitmap.height as usize / 2) * bitmap.width as usize
            + bitmap.width as usize / 2];
        assert_ne!(center >> 24, 0, "the clipped gradient should cover the middle");
        let corner = bitmap.pixels[0];
        assert_eq!(corner >> 24, 0, "the clip should leave the corner clear");
    }

    #[test]
    fn encodes_a_png_round_trip() {
        let png = encode_png(1, 1, &[255, 0, 0]).expect("png");
        let bitmap = decode(&png).expect("decode");
        assert_eq!(bitmap.width, 1);
        assert_eq!(bitmap.pixels[0] & 0x00FF_FFFF, 0x00FF_0000);
    }
}
