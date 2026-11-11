//! Decode PNG, JPEG, GIF, and WebP into pixels the painter can blend.
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
    fn encodes_a_png_round_trip() {
        let png = encode_png(1, 1, &[255, 0, 0]).expect("png");
        let bitmap = decode(&png).expect("decode");
        assert_eq!(bitmap.width, 1);
        assert_eq!(bitmap.pixels[0] & 0x00FF_FFFF, 0x00FF_0000);
    }
}
