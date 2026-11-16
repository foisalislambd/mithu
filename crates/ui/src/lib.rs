//! Desktop frame shared by the Servo build and Mithu Lite.
//!
//! The tab strip, toolbar, omnibox, bookmarks bar, status bubble, and new-tab
//! page are painted here. Lucide icons are rasterized in [`icons`]. Neither
//! build draws a Google or Chrome mark.

use url::Url;

pub mod chrome;
pub mod icons;

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
    Url::parse(&format!("https://duckduckgo.com/?q={}", percent(text))).expect("search")
}

fn percent(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
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
            tabs: vec![chrome::TabPaint { title: "New Tab".into(), favicon: None }],
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
        };
        assert_eq!(chrome::content_chrome(&state), TAB_H + TOOLBAR_H + BOOKMARK_BAR_H);
        let mut pixels = vec![0u32; 1280 * 800];
        chrome::paint(&mut pixels, 1280, 800, &state);
        assert!(pixels.iter().any(|pixel| *pixel != 0));
    }
}
