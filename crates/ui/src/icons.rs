//! Frame icons rasterized with resvg.
//!
//! The SVGs in `icons/chromium/` are converted one-to-one from Chromium's
//! `.icon` vector sources (BSD-3-Clause; see `icons/NOTICE`). They are drawn
//! on Chromium's 16 or 20 DIP grid, so they are rasterized straight at the
//! destination size, as Skia does, which keeps the strokes pixel-aligned.

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
    fn source(self) -> &'static str {
        match self {
            Kind::Back => include_str!("../icons/chromium/back_arrow_20.svg"),
            Kind::Forward => include_str!("../icons/chromium/forward_arrow_20.svg"),
            Kind::Reload => include_str!("../icons/chromium/reload_20.svg"),
            Kind::Stop => include_str!("../icons/chromium/navigate_stop_20.svg"),
            Kind::Home => include_str!("../icons/chromium/navigate_home_20.svg"),
            Kind::Lock => include_str!("../icons/chromium/lock_24.svg"),
            Kind::Star => include_str!("../icons/chromium/star_20.svg"),
            Kind::StarFilled => include_str!("../icons/chromium/star_active_20.svg"),
            Kind::Extensions => include_str!("../icons/chromium/extension_20.svg"),
            Kind::Menu => include_str!("../icons/chromium/browser_tools_20.svg"),
            Kind::Plus => include_str!("../icons/chromium/add_16.svg"),
            Kind::Search => include_str!("../icons/chromium/search_20.svg"),
            Kind::Minimize => include_str!("../icons/window/minimize.svg"),
            Kind::Maximize => include_str!("../icons/window/maximize.svg"),
            Kind::Restore => include_str!("../icons/window/restore.svg"),
            Kind::Close => include_str!("../icons/window/close.svg"),
            Kind::Download => include_str!("../icons/chromium/file_download_20.svg"),
            Kind::History => include_str!("../icons/chromium/history_20.svg"),
            Kind::Settings => include_str!("../icons/chromium/settings_20.svg"),
            Kind::Globe => include_str!("../icons/chromium/globe_16.svg"),
            Kind::CloseTab => include_str!("../icons/chromium/close_tab_16.svg"),
            Kind::TabSearch => include_str!("../icons/chromium/keyboard_arrow_down_20.svg"),
            Kind::Avatar => include_str!("../icons/chromium/user_account_avatar_20.svg"),
            Kind::PageInfo => include_str!("../icons/chromium/secure_page_info_16.svg"),
            Kind::NotSecure => include_str!("../icons/chromium/not_secure_warning_16.svg"),
            Kind::Info => include_str!("../icons/chromium/http_16.svg"),
            Kind::Folder => include_str!("../icons/chromium/folder_20.svg"),
            Kind::NewTabPage => include_str!("../icons/chromium/new_tab_16.svg"),
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
