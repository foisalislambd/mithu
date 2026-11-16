//! Lucide icons rasterized with resvg.
//!
//! The SVGs live in `crates/shell/icons/` (lucide-static 0.544.0, ISC).
//! Each glyph is drawn at twice the destination size and box-filtered back
//! down so the 2px stroke stays crisp on a 1x framebuffer.

use std::collections::HashMap;
use std::sync::Mutex;

use resvg::tiny_skia::{self, Pixmap};
use resvg::usvg::{self, Tree};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    Back,
    Forward,
    Reload,
    Stop,
    Home,
    Lock,
    Star,
    StarFilled,
    Extensions,
    Menu,
    Plus,
    Search,
    Minimize,
    Maximize,
    Close,
    Download,
    History,
    Settings,
    Globe,
}

pub fn blit(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    size: f32,
    kind: Kind,
    color: u32,
) {
    let px = size.round().max(1.0) as u32;
    let bitmap = raster(kind, color & 0x00ff_ffff, px);
    let left = x.round() as i32;
    let top = y.round() as i32;
    for row in 0..px {
        for col in 0..px {
            let index = ((row * px + col) * 4) as usize;
            let alpha = bitmap[index + 3];
            if alpha == 0 {
                continue;
            }
            let unscale = 255u32 / alpha as u32;
            let red = (bitmap[index] as u32 * unscale).min(255);
            let green = (bitmap[index + 1] as u32 * unscale).min(255);
            let blue = (bitmap[index + 2] as u32 * unscale).min(255);
            blend(
                pixels,
                width,
                height,
                left + col as i32,
                top + row as i32,
                (red << 16) | (green << 8) | blue,
                alpha,
            );
        }
    }
}

fn raster(kind: Kind, color: u32, size: u32) -> Vec<u8> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<HashMap<(u8, u32, u32), Vec<u8>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (kind as u8, color, size);
    if let Some(hit) = cache.lock().expect("icon cache").get(&key) {
        return hit.clone();
    }
    let painted = paint_svg(kind.source(), color, size);
    cache.lock().expect("icon cache").insert(key, painted.clone());
    painted
}

fn paint_svg(svg: &str, color: u32, size: u32) -> Vec<u8> {
    let hex = format!("#{color:06x}");
    let svg = svg.replace("currentColor", &hex);
    let tree = Tree::from_str(&svg, &usvg::Options::default()).unwrap_or_else(|_| {
        Tree::from_str(Kind::Close.source(), &usvg::Options::default()).expect("close icon")
    });
    let scale = (size as f32 * 2.0) / tree.size().width().max(1.0);
    let big = size.saturating_mul(2).max(2);
    let mut pixmap = Pixmap::new(big, big).unwrap_or_else(|| Pixmap::new(2, 2).expect("pixmap"));
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    downsample(pixmap.data(), big, size)
}

fn downsample(src: &[u8], src_size: u32, dst_size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dst_size * dst_size * 4) as usize];
    if src_size == 0 || dst_size == 0 {
        return out;
    }
    for row in 0..dst_size {
        for col in 0..dst_size {
            let x0 = col * src_size / dst_size;
            let x1 = ((col + 1) * src_size / dst_size).max(x0 + 1).min(src_size);
            let y0 = row * src_size / dst_size;
            let y1 = ((row + 1) * src_size / dst_size).max(y0 + 1).min(src_size);
            let mut red = 0u32;
            let mut green = 0u32;
            let mut blue = 0u32;
            let mut alpha = 0u32;
            let mut count = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let index = ((y * src_size + x) * 4) as usize;
                    if index + 3 >= src.len() {
                        continue;
                    }
                    red += src[index] as u32;
                    green += src[index + 1] as u32;
                    blue += src[index + 2] as u32;
                    alpha += src[index + 3] as u32;
                    count += 1;
                }
            }
            if count == 0 {
                continue;
            }
            let index = ((row * dst_size + col) * 4) as usize;
            out[index] = (red / count) as u8;
            out[index + 1] = (green / count) as u8;
            out[index + 2] = (blue / count) as u8;
            out[index + 3] = (alpha / count) as u8;
        }
    }
    out
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
    *dest = (((sr * a + dr * inv) / 255) << 16)
        | (((sg * a + dg * inv) / 255) << 8)
        | ((sb * a + db * inv) / 255);
}

impl Kind {
    fn source(self) -> &'static str {
        match self {
            Kind::Back => include_str!("../icons/arrow-left.svg"),
            Kind::Forward => include_str!("../icons/arrow-right.svg"),
            Kind::Reload => include_str!("../icons/rotate-cw.svg"),
            Kind::Stop => include_str!("../icons/x.svg"),
            Kind::Home => include_str!("../icons/house.svg"),
            Kind::Lock => include_str!("../icons/lock.svg"),
            Kind::Star => include_str!("../icons/star.svg"),
            Kind::StarFilled => include_str!("../icons/star-filled.svg"),
            Kind::Extensions => include_str!("../icons/puzzle.svg"),
            Kind::Menu => include_str!("../icons/ellipsis-vertical.svg"),
            Kind::Plus => include_str!("../icons/plus.svg"),
            Kind::Search => include_str!("../icons/search.svg"),
            Kind::Minimize => include_str!("../icons/minus.svg"),
            Kind::Maximize => include_str!("../icons/square.svg"),
            Kind::Close => include_str!("../icons/x.svg"),
            Kind::Download => include_str!("../icons/download.svg"),
            Kind::History => include_str!("../icons/history.svg"),
            Kind::Settings => include_str!("../icons/settings.svg"),
            Kind::Globe => include_str!("../icons/globe.svg"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_icon_has_ink() {
        let bitmap = paint_svg(Kind::Back.source(), 0x1f1f1f, 16);
        assert_eq!(bitmap.len(), 16 * 16 * 4);
        assert!(bitmap.iter().any(|byte| *byte != 0));
    }
}
