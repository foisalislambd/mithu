//! Frame icons rasterized with resvg.
//!
//! The SVGs in `icons/lucide/` are the Lucide outline set (ISC; see
//! `icons/lucide/LICENSE` and `icons/NOTICE`). Each icon is a 24×24 vector,
//! rasterized at the destination pixel size so strokes stay sharp at 1× and
//! 2×.

use std::collections::HashMap;
use std::sync::Mutex;

use resvg::tiny_skia::{self, Pixmap};
use resvg::usvg::{self, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// Tab close glyph (16 DIP grid).
    CloseTab,
    /// Tab search caret at the leading edge of the tab strip.
    TabSearch,
    /// Signed-out avatar.
    Avatar,
    /// Page-info "tune" glyph shown for HTTPS pages.
    PageInfo,
    /// Warning triangle for "Not secure".
    NotSecure,
    /// Info glyph for local and internal pages.
    Info,
    Folder,
    NewTabPage,
    Restore,
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
    let painted = paint_svg(kind.svg(), color, size);
    cache.lock().expect("icon cache").insert(key, painted.clone());
    painted
}

/// Rasterize an SVG that uses `currentColor` into straight RGBA.
pub(crate) fn raster_svg(svg: &str, color: u32, size: u32) -> (u32, u32, Vec<u8>) {
    let size = size.max(1);
    let bytes = paint_svg(svg, color, size);
    (size, size, bytes)
}

fn paint_svg(svg: &str, color: u32, size: u32) -> Vec<u8> {
    let hex = format!("#{color:06x}");
    let svg = svg.replace("currentColor", &hex);
    let tree = Tree::from_str(&svg, &usvg::Options::default()).unwrap_or_else(|_| {
        Tree::from_str(Kind::Close.svg(), &usvg::Options::default()).expect("close icon")
    });
    let size = size.max(1);
    let scale = size as f32 / tree.size().width().max(1.0);
    let mut pixmap = Pixmap::new(size, size).unwrap_or_else(|| Pixmap::new(1, 1).expect("pixmap"));
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.data().to_vec()
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
    /// Lucide SVG using `currentColor`. Safe to embed in internal pages.
    pub fn svg(self) -> &'static str {
        match self {
            Kind::Back => include_str!("../icons/lucide/arrow-left.svg"),
            Kind::Forward => include_str!("../icons/lucide/arrow-right.svg"),
            Kind::Reload => include_str!("../icons/lucide/rotate-cw.svg"),
            Kind::Stop => include_str!("../icons/lucide/x.svg"),
            Kind::Home => include_str!("../icons/lucide/house.svg"),
            Kind::Lock => include_str!("../icons/lucide/lock.svg"),
            Kind::Star => include_str!("../icons/lucide/star.svg"),
            Kind::StarFilled => include_str!("../icons/lucide/star-filled.svg"),
            Kind::Extensions => include_str!("../icons/lucide/puzzle.svg"),
            Kind::Menu => include_str!("../icons/lucide/ellipsis-vertical.svg"),
            Kind::Plus => include_str!("../icons/lucide/plus.svg"),
            Kind::Search => include_str!("../icons/lucide/search.svg"),
            Kind::Minimize => include_str!("../icons/lucide/minus.svg"),
            Kind::Maximize => include_str!("../icons/lucide/square.svg"),
            Kind::Restore => include_str!("../icons/lucide/restore.svg"),
            Kind::Close => include_str!("../icons/lucide/x.svg"),
            Kind::Download => include_str!("../icons/lucide/download.svg"),
            Kind::History => include_str!("../icons/lucide/history.svg"),
            Kind::Settings => include_str!("../icons/lucide/settings.svg"),
            Kind::Globe => include_str!("../icons/lucide/globe.svg"),
            Kind::CloseTab => include_str!("../icons/lucide/x.svg"),
            Kind::TabSearch => include_str!("../icons/lucide/chevron-down.svg"),
            Kind::Avatar => include_str!("../icons/lucide/circle-user.svg"),
            Kind::PageInfo => include_str!("../icons/lucide/sliders-horizontal.svg"),
            Kind::NotSecure => include_str!("../icons/lucide/triangle-alert.svg"),
            Kind::Info => include_str!("../icons/lucide/info.svg"),
            Kind::Folder => include_str!("../icons/lucide/folder.svg"),
            Kind::NewTabPage => include_str!("../icons/lucide/square-plus.svg"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_icon_has_ink() {
        let bitmap = paint_svg(Kind::Back.svg(), 0x1f1f1f, 16);
        assert_eq!(bitmap.len(), 16 * 16 * 4);
        assert!(bitmap.iter().any(|byte| *byte != 0));
    }

    #[test]
    fn outline_icons_have_ink_at_hidpi() {
        for kind in [
            Kind::Back,
            Kind::Forward,
            Kind::Reload,
            Kind::Home,
            Kind::Star,
            Kind::StarFilled,
            Kind::Menu,
            Kind::Search,
            Kind::Settings,
            Kind::Download,
            Kind::Close,
            Kind::NewTabPage,
        ] {
            let bitmap = paint_svg(kind.svg(), 0x1f1f1f, 32);
            assert_eq!(bitmap.len(), 32 * 32 * 4, "{kind:?}");
            assert!(bitmap.iter().any(|byte| *byte != 0), "{kind:?}");
            assert!(kind.svg().contains("currentColor"), "{kind:?}");
        }
        assert_ne!(Kind::Star.svg(), Kind::StarFilled.svg());
    }
}
