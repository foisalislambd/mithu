//! Desktop frame shared by the Servo build and Mithu Lite.
//!
//! The tab strip, toolbar, omnibox, bookmarks bar, status bubble, and new-tab
//! page are painted here. Lucide icons are rasterized in [`icons`]. Neither
//! build draws a Google or Chrome mark.

use url::Url;

pub mod brand;
pub mod chrome;
pub mod icons;
pub mod pages;

pub const TAB_H: f32 = 41.0;
pub const TOOLBAR_H: f32 = 48.0;
pub const BOOKMARK_BAR_H: f32 = 34.0;
pub const CAPTION_W: f32 = 46.0;
pub const BTN: f32 = 34.0;

pub const TAB_STRIP: u32 = rgb(211, 227, 253);
pub const TOOLBAR: u32 = rgb(255, 255, 255);
pub const ICON: u32 = rgb(31, 31, 31);
pub const ICON_OFF: u32 = rgb(143, 143, 143);
pub const OMNIBOX: u32 = rgb(239, 237, 237);
pub const ADDRESS: u32 = rgb(255, 255, 255);
pub const ADDRESS_BORDER: u32 = rgb(199, 199, 199);
pub const ADDRESS_FOCUS: u32 = rgb(11, 87, 208);
pub const INK: u32 = rgb(31, 31, 31);
pub const MUTED: u32 = rgb(94, 94, 94);
pub const HOVER: u32 = rgb(168, 199, 250);
pub const PLUS: u32 = rgb(236, 243, 254);
pub const STAR: u32 = rgb(242, 180, 0);
pub const PAGE: u32 = rgb(244, 241, 234);

pub const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    (r << 16) | (g << 8) | b
}

/// Turn omnibox text into a URL. A hostname without a scheme is HTTPS.
/// Other text is a DuckDuckGo query. No Google host is used.
pub fn parse_location(input: &str) -> Url {
    let text = input.trim();
    if text.is_empty() {
        return Url::parse("about:blank").expect("blank");
    }
    if text.starts_with("about:")
        || text.starts_with("data:")
        || text.starts_with("file:")
        || text.contains("://")
    {
        return Url::parse(text).unwrap_or_else(|_| Url::parse("about:blank").expect("blank"));
    }
    if !text.contains(' ') && text.contains('.') {
        return Url::parse(&format!("https://{text}"))
            .unwrap_or_else(|_| Url::parse("about:blank").expect("blank"));
    }
    pages::search_url(&search_engine(), text)
}

fn search_engine() -> String {
    SEARCH_ENGINE.with(|engine| engine.borrow().clone())
}

thread_local! {
    static SEARCH_ENGINE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Address-bar search used by both editions. Empty or unknown keeps DuckDuckGo.
pub fn set_search_engine(name: &str) {
    let name = match name {
        "bing" | "wikipedia" | "duckduckgo" => name.to_string(),
        _ => "duckduckgo".into(),
    };
    SEARCH_ENGINE.with(|engine| *engine.borrow_mut() = name);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_parser_uses_https_and_not_a_google_host() {
        let url = parse_location("example.com");
        assert_eq!(url.as_str(), "https://example.com/");
        let search = parse_location("mithu browser");
        assert_eq!(search.domain(), Some("duckduckgo.com"));
    }

    #[test]
    fn chrome_height_includes_the_bookmarks_bar() {
        let fonts = mithu_text::FontBook::load();
        let state = chrome::State {
            width: 1280.0,
            height: 800.0,
            tabs: vec![chrome::TabPaint {
                title: "New Tab".into(),
                favicon: None,
            }],
            active: 0,
            omnibox: String::new(),
            omnibox_focused: false,
            cursor: 0,
            secure: chrome::Secure::Internal,
            bookmarked: false,
            bookmarks: Vec::new(),
            bookmarks_open: true,
            can_back: false,
            can_forward: false,
            loading: false,
            progress: 0.0,
            link_status: String::new(),
            pointer: (-1.0, -1.0),
            pressed: false,
            ntp: true,
            menu: None,
            menu_labels: Vec::new(),
            tab_search: false,
            extensions: false,
            profile: false,
            suggestions: Vec::new(),
            suggestion: None,
            find_open: false,
            find_query: String::new(),
            fonts: &fonts,
            scale: 1.0,
            dark: false,
            show_home: true,
        };
        assert_eq!(
            chrome::content_chrome(&state),
            TAB_H + TOOLBAR_H + BOOKMARK_BAR_H
        );
        let mut pixels = vec![0u32; 1280 * 800];
        chrome::paint(&mut pixels, 1280, 800, &state);
        assert!(pixels.iter().any(|pixel| *pixel != 0));
    }

    #[test]
    fn bundled_fonts_paint_the_new_tab_title() {
        let fonts = mithu_text::FontBook::bundled();
        let state = chrome::State {
            width: 1280.0,
            height: 800.0,
            tabs: vec![chrome::TabPaint {
                title: "New Tab".into(),
                favicon: None,
            }],
            active: 0,
            omnibox: String::new(),
            omnibox_focused: false,
            cursor: 0,
            secure: chrome::Secure::Internal,
            bookmarked: false,
            bookmarks: Vec::new(),
            bookmarks_open: true,
            can_back: false,
            can_forward: false,
            loading: false,
            progress: 0.0,
            link_status: String::new(),
            pointer: (-1.0, -1.0),
            pressed: false,
            ntp: true,
            menu: None,
            menu_labels: Vec::new(),
            tab_search: false,
            extensions: false,
            profile: false,
            suggestions: Vec::new(),
            suggestion: None,
            find_open: false,
            find_query: String::new(),
            fonts: &fonts,
            scale: 1.0,
            dark: false,
            show_home: true,
        };
        let mut pixels = vec![0u32; 1280 * 800];
        chrome::paint(&mut pixels, 1280, 800, &state);
        // The word "Mithu" is drawn just under the new-tab mark. A missing
        // font fills that band with solid boxes, which have no light pixels
        // inside the ink.
        let mut dark = 0u32;
        let mut light_beside_dark = 0u32;
        for y in 320..380 {
            for x in 520..760 {
                let pixel = pixels[(y * 1280 + x) as usize];
                let dark_pixel = (pixel & 0xff) < 80;
                if dark_pixel {
                    dark += 1;
                    let right = pixels[(y * 1280 + x + 1) as usize];
                    if (right & 0xff) > 200 {
                        light_beside_dark += 1;
                    }
                }
            }
        }
        assert!(dark > 40, "no title ink, dark={dark}");
        assert!(
            light_beside_dark > 10,
            "title ink has no gaps, so the glyphs are boxes; edges={light_beside_dark}"
        );
        if let Ok(path) = std::env::var("MITHU_FRAME") {
            let mut ppm = format!("P6\n1280 800\n255\n").into_bytes();
            for pixel in &pixels {
                ppm.push(((pixel >> 16) & 255) as u8);
                ppm.push(((pixel >> 8) & 255) as u8);
                ppm.push((pixel & 255) as u8);
            }
            std::fs::write(path, ppm).expect("write frame");
        }
    }

    fn sample(fonts: &mithu_text::FontBook, scale: f32, dark: bool) -> chrome::State<'_> {
        chrome::State {
            width: 1100.0 * scale,
            height: 720.0 * scale,
            tabs: vec![
                chrome::TabPaint { title: "New Tab".into(), favicon: None },
                chrome::TabPaint { title: "Mithu notes".into(), favicon: None },
                chrome::TabPaint { title: "A longer page title".into(), favicon: None },
            ],
            active: 0,
            omnibox: String::new(),
            omnibox_focused: false,
            cursor: 0,
            secure: chrome::Secure::Https,
            bookmarked: true,
            bookmarks: vec![
                ("Notes".into(), "https://example.com/notes".into()),
                ("Maps".into(), "https://example.com/maps".into()),
            ],
            bookmarks_open: true,
            can_back: true,
            can_forward: false,
            loading: true,
            progress: 0.4,
            link_status: "https://example.com/notes".into(),
            pointer: (180.0 * scale, 70.0 * scale),
            pressed: false,
            ntp: true,
            menu: Some(chrome::Popup::App),
            menu_labels: vec!["New tab".into(), "History".into(), "Downloads".into(), "Settings".into()],
            tab_search: false,
            extensions: false,
            profile: false,
            suggestions: Vec::new(),
            suggestion: None,
            find_open: false,
            find_query: String::new(),
            fonts,
            scale,
            dark,
            show_home: true,
        }
    }

    #[test]
    fn write_ui_contact_sheets() {
        let Ok(dir) = std::env::var("MITHU_UI_DIR") else {
            return;
        };
        let fonts = mithu_text::FontBook::bundled();
        for (name, scale, dark) in [
            ("100", 1.0, false),
            ("125", 1.25, false),
            ("150", 1.5, false),
            ("200", 2.0, false),
            ("100-dark", 1.0, true),
        ] {
            let state = sample(&fonts, scale, dark);
            let w = state.width.round() as u32;
            let h = state.height.round() as u32;
            let mut pixels = vec![0u32; (w * h) as usize];
            chrome::paint(&mut pixels, w, h, &state);
            let path = std::path::Path::new(&dir).join(format!("frame-{name}.ppm"));
            write_ppm(&path, &pixels, w, h);
        }
    }

    fn write_ppm(path: &std::path::Path, pixels: &[u32], w: u32, h: u32) {
        let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
        for pixel in pixels {
            ppm.push(((pixel >> 16) & 255) as u8);
            ppm.push(((pixel >> 8) & 255) as u8);
            ppm.push((pixel & 255) as u8);
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, ppm).expect("write ppm");
    }
}
