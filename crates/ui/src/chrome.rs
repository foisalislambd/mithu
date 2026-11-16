//! Desktop frame painted in the style of the current Chrome window, without
//! any Google or Chrome mark. Icons are Lucide; the wordmark is "Mithu".

use mithu_text::{glyph_top, FontBook, Glyph};

use crate::icons::{self, Kind};
use crate::{
    ADDRESS_FOCUS, BOOKMARK_BAR_H, BTN, CAPTION_W, HOVER, ICON, ICON_OFF, INK, MUTED, OMNIBOX,
    PLUS, STAR, TAB_H, TAB_STRIP, TOOLBAR, TOOLBAR_H,
};

const CLOSE_HOVER: u32 = rgb(196, 43, 28);
const NOT_SECURE: u32 = rgb(95, 99, 104);
const CHIP: u32 = rgb(232, 234, 237);
const BUBBLE: u32 = rgb(242, 242, 242);
const LINE: u32 = rgb(218, 220, 224);
const PROGRESS: u32 = rgb(26, 115, 232);
const PROFILE: u32 = rgb(11, 87, 208);

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    (r << 16) | (g << 8) | b
}

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    fn right(self) -> f32 {
        self.x + self.w
    }

    fn bottom(self) -> f32 {
        self.y + self.h
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Secure {
    Https,
    Http,
    Internal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    App,
    Context,
}

pub struct TabPaint {
    pub title: String,
    pub favicon: Option<(u32, u32, Vec<u8>)>,
}

pub struct State<'a> {
    pub width: f32,
    pub height: f32,
    pub tabs: Vec<TabPaint>,
    pub active: usize,
    pub omnibox: String,
    pub omnibox_focused: bool,
    pub cursor: usize,
    pub secure: Secure,
    pub bookmarked: bool,
    pub bookmarks: Vec<(String, String)>,
    pub bookmarks_open: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub loading: bool,
    pub progress: f32,
    pub link_status: String,
    pub pointer: (f32, f32),
    pub pressed: bool,
    pub ntp: bool,
    pub menu: Option<Popup>,
    pub menu_labels: Vec<String>,
    pub tab_search: bool,
    pub extensions: bool,
    pub profile: bool,
    pub suggestions: Vec<String>,
    pub suggestion: Option<usize>,
    pub find_open: bool,
    pub find_query: String,
    pub fonts: &'a FontBook,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Drag,
    Minimize,
    Maximize,
    Close,
    Tab(usize),
    CloseTab(usize),
    NewTab,
    TabSearch,
    Back,
    Forward,
    Reload,
    Home,
    Omnibox,
    Star,
    Extensions,
    Profile,
    Menu,
    Bookmark(usize),
    MenuItem(usize),
    Suggestion(usize),
    Panel,
    Content,
    None,
}

struct TabGeom {
    rect: Rect,
    close: Rect,
    title_left: f32,
    title_right: f32,
}

struct Tools {
    back: Rect,
    forward: Rect,
    reload: Rect,
    home: Rect,
    address: Rect,
    star: Rect,
    extensions: Rect,
    profile: Rect,
    menu: Rect,
}

pub fn paint(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let w = state.width;
    fill_rect(pixels, width, height, 0.0, 0.0, w, TAB_H, TAB_STRIP);
    fill_rect(pixels, width, height, 0.0, TAB_H, w, TOOLBAR_H, TOOLBAR);
    let bar = content_chrome(state);
    if state.bookmarks_open {
        fill_rect(pixels, width, height, 0.0, TAB_H + TOOLBAR_H, w, BOOKMARK_BAR_H, TOOLBAR);
    }
    fill_rect(pixels, width, height, 0.0, bar - 1.0, w, 1.0, LINE);

    let (tabs, plus, search) = tab_layout(w, state.tabs.len().max(1));
    for (index, geom) in tabs.iter().enumerate() {
        let active = index == state.active;
        let hovered = geom.rect.contains(state.pointer.0, state.pointer.1);
        if active {
            fill_polygon(pixels, width, height, &active_tab(geom.rect), TOOLBAR);
        } else if hovered {
            fill_round(pixels, width, height, geom.rect.x, geom.rect.y, geom.rect.w, geom.rect.h - 4.0, 8.0, PLUS);
        } else if index + 1 != state.active && index != state.active.saturating_add(1) {
            let sep_x = geom.rect.right() + 1.0;
            fill_rect(pixels, width, height, sep_x, 14.0, 1.0, 16.0, rgb(160, 180, 210));
        }
        let title = state.tabs.get(index).map(|tab| tab.title.as_str()).unwrap_or("New Tab");
        if let Some((fw, fh, bytes)) = state.tabs.get(index).and_then(|tab| tab.favicon.as_ref()) {
            blit_favicon(pixels, width, height, geom.rect.x + 12.0, geom.rect.y + 9.0, *fw, *fh, bytes);
        } else {
            icons::blit(pixels, width, height, geom.rect.x + 12.0, geom.rect.y + 9.0, 16.0, Kind::Globe, ICON_OFF);
        }
        if geom.title_right - geom.title_left > 8.0 {
            draw_text_fade(
                pixels,
                width,
                height,
                state.fonts,
                geom.title_left,
                geom.rect.y + 22.0,
                12.0,
                INK,
                title,
                geom.title_left,
                geom.title_right,
                16.0,
            );
        }
        let show_close = active || geom.rect.contains(state.pointer.0, state.pointer.1);
        if show_close && geom.rect.w >= 36.0 {
            let over = geom.close.contains(state.pointer.0, state.pointer.1);
            if over {
                fill_circle(pixels, width, height, geom.close.x + 8.0, geom.close.y + 8.0, 10.0, CLOSE_HOVER);
            }
            icons::blit(
                pixels,
                width,
                height,
                geom.close.x,
                geom.close.y,
                16.0,
                Kind::Close,
                if over { TOOLBAR } else { ICON },
            );
        }
    }
    button_disc(pixels, width, height, plus, state, true);
    icons::blit(pixels, width, height, plus.x + 6.0, plus.y + 6.0, 16.0, Kind::Plus, ICON);
    button_disc(pixels, width, height, search, state, true);
    icons::blit(pixels, width, height, search.x + 6.0, search.y + 6.0, 16.0, Kind::Search, ICON);
    paint_captions(pixels, width, height, state);
    tip(pixels, width, height, state, plus, "New tab");
    tip(pixels, width, height, state, search, "Search tabs");

    let tools = toolbar(w);
    tool_icon(pixels, width, height, state, tools.back, Kind::Back, state.can_back, "Back");
    tool_icon(pixels, width, height, state, tools.forward, Kind::Forward, state.can_forward, "Forward");
    let reload_kind = if state.loading { Kind::Stop } else { Kind::Reload };
    let reload_tip = if state.loading { "Stop" } else { "Reload" };
    tool_icon(pixels, width, height, state, tools.reload, reload_kind, true, reload_tip);
    tool_icon(pixels, width, height, state, tools.home, Kind::Home, true, "Home");

    fill_round(
        pixels,
        width,
        height,
        tools.address.x,
        tools.address.y,
        tools.address.w,
        tools.address.h,
        tools.address.h / 2.0,
        OMNIBOX,
    );
    if state.omnibox_focused {
        stroke_round(
            pixels,
            width,
            height,
            tools.address.x + 1.0,
            tools.address.y + 1.0,
            tools.address.w - 2.0,
            tools.address.h - 2.0,
            (tools.address.h - 2.0) / 2.0,
            ADDRESS_FOCUS,
        );
    }
    let mut text_left = tools.address.x + 14.0;
    match state.secure {
        Secure::Https => {
            icons::blit(pixels, width, height, text_left, tools.address.y + 9.0, 16.0, Kind::Lock, ICON);
            text_left += 24.0;
        }
        Secure::Http => {
            let label = "Not secure";
            draw_text(pixels, width, height, state.fonts, text_left, tools.address.y + 22.0, 13.0, NOT_SECURE, label);
            text_left += text_width(state.fonts, label, 13.0) + 10.0;
        }
        Secure::Internal => {}
    }
    let shown = omnibox_text(&state.omnibox);
    let text_right = tools.star.x - 4.0;
    if shown.is_empty() && !state.omnibox_focused {
        draw_text_fade(
            pixels,
            width,
            height,
            state.fonts,
            text_left,
            tools.address.y + 22.0,
            14.0,
            ICON_OFF,
            "Search the web or type a URL",
            text_left,
            text_right,
            18.0,
        );
    } else {
        draw_text_fade(
            pixels,
            width,
            height,
            state.fonts,
            text_left,
            tools.address.y + 22.0,
            14.0,
            INK,
            shown,
            text_left,
            text_right,
            18.0,
        );
        if state.omnibox_focused {
            let caret = text_left + text_width(state.fonts, &shown.chars().take(state.cursor).collect::<String>(), 14.0);
            if caret < text_right {
                fill_rect(pixels, width, height, caret, tools.address.y + 8.0, 1.0, 18.0, INK);
            }
        }
    }
    button_disc(pixels, width, height, tools.star, state, true);
    icons::blit(
        pixels,
        width,
        height,
        tools.star.x + 9.0,
        tools.star.y + 9.0,
        16.0,
        if state.bookmarked { Kind::StarFilled } else { Kind::Star },
        if state.bookmarked { STAR } else { ICON },
    );
    tip(pixels, width, height, state, tools.star, "Bookmark this tab");
    tool_icon(pixels, width, height, state, tools.extensions, Kind::Extensions, true, "Extensions");
    paint_profile(pixels, width, height, state, tools.profile);
    tip(pixels, width, height, state, tools.profile, "Profile");
    tool_icon(pixels, width, height, state, tools.menu, Kind::Menu, true, "Customize and control Mithu");

    if state.bookmarks_open {
        paint_bookmarks(pixels, width, height, state);
    }
    if state.loading && state.progress > 0.0 {
        let top = content_chrome(state);
        fill_rect(pixels, width, height, 0.0, top - 2.0, w * state.progress.clamp(0.0, 1.0), 2.0, PROGRESS);
    }
    if state.ntp {
        paint_ntp(pixels, width, height, state);
    }
    if state.find_open {
        paint_find(pixels, width, height, state);
    }
    if state.omnibox_focused && !state.suggestions.is_empty() {
        paint_suggestions(pixels, width, height, state, tools.address);
    }
    if state.tab_search {
        paint_tab_search(pixels, width, height, state, search);
    }
    if state.extensions {
        paint_extensions(pixels, width, height, state, tools.extensions);
    }
    if state.profile {
        paint_profile_card(pixels, width, height, state, tools.profile);
    }
    if let Some(popup) = state.menu {
        paint_menu(pixels, width, height, state, popup);
    }
    paint_status(pixels, width, height, state);
}

pub fn hit(state: &State<'_>, x: f32, y: f32) -> Hit {
    let tools = toolbar(state.width);
    if let Some(popup) = state.menu {
        let rect = menu_rect(state, popup);
        if rect.contains(x, y) {
            let index = ((y - rect.y - 8.0) / 32.0).floor() as isize;
            if index >= 0 && (index as usize) < state.menu_labels.len() {
                return Hit::MenuItem(index as usize);
            }
            return Hit::Panel;
        }
    }
    if state.tab_search {
        let (_, _, search) = tab_layout(state.width, state.tabs.len().max(1));
        let rect = Rect { x: search.x, y: search.bottom() + 6.0, w: 280.0, h: 12.0 + state.tabs.len().max(1) as f32 * 32.0 };
        if rect.contains(x, y) {
            let index = ((y - rect.y - 6.0) / 32.0).floor() as isize;
            if index >= 0 && (index as usize) < state.tabs.len() {
                return Hit::Tab(index as usize);
            }
            return Hit::Panel;
        }
    }
    if state.extensions {
        if extensions_rect(state).contains(x, y) {
            return Hit::Panel;
        }
    }
    if state.profile {
        let rect = profile_rect(state);
        if rect.contains(x, y) {
            return Hit::Panel;
        }
    }
    if state.omnibox_focused && !state.suggestions.is_empty() {
        let rect = Rect {
            x: tools.address.x,
            y: tools.address.bottom() + 4.0,
            w: tools.address.w,
            h: state.suggestions.len() as f32 * 32.0 + 8.0,
        };
        if rect.contains(x, y) {
            let index = ((y - rect.y - 4.0) / 32.0).floor() as isize;
            if index >= 0 && (index as usize) < state.suggestions.len() {
                return Hit::Suggestion(index as usize);
            }
            return Hit::Panel;
        }
    }
    if y < TAB_H {
        if let Some(caption) = caption_at(state.width, x) {
            return caption;
        }
        let (tabs, plus, search) = tab_layout(state.width, state.tabs.len().max(1));
        if plus.contains(x, y) {
            return Hit::NewTab;
        }
        if search.contains(x, y) {
            return Hit::TabSearch;
        }
        for (index, geom) in tabs.iter().enumerate() {
            if geom.close.contains(x, y) && geom.rect.w >= 36.0 {
                return Hit::CloseTab(index);
            }
            if geom.rect.contains(x, y) {
                return Hit::Tab(index);
            }
        }
        return Hit::Drag;
    }
    if y < TAB_H + TOOLBAR_H {
        if tools.back.contains(x, y) && state.can_back {
            return Hit::Back;
        }
        if tools.forward.contains(x, y) && state.can_forward {
            return Hit::Forward;
        }
        if tools.reload.contains(x, y) {
            return Hit::Reload;
        }
        if tools.home.contains(x, y) {
            return Hit::Home;
        }
        if tools.star.contains(x, y) {
            return Hit::Star;
        }
        if tools.extensions.contains(x, y) {
            return Hit::Extensions;
        }
        if tools.profile.contains(x, y) {
            return Hit::Profile;
        }
        if tools.menu.contains(x, y) {
            return Hit::Menu;
        }
        if tools.address.contains(x, y) {
            return Hit::Omnibox;
        }
        return Hit::None;
    }
    if state.bookmarks_open && y < TAB_H + TOOLBAR_H + BOOKMARK_BAR_H {
        for (index, rect) in bookmark_rects(state).into_iter().enumerate() {
            if rect.contains(x, y) {
                return Hit::Bookmark(index);
            }
        }
        return Hit::None;
    }
    if y >= content_chrome(state) {
        Hit::Content
    } else {
        Hit::None
    }
}

pub fn content_chrome(state: &State<'_>) -> f32 {
    let mut top = TAB_H + TOOLBAR_H;
    if state.bookmarks_open {
        top += BOOKMARK_BAR_H;
    }
    top
}

fn tab_layout(width: f32, count: usize) -> (Vec<TabGeom>, Rect, Rect) {
    let count = count.max(1);
    let left = 8.0;
    let reserved = 8.0 + 28.0 + 4.0 + 28.0 + 12.0 + CAPTION_W * 3.0;
    let available = (width - left - reserved).max(16.0);
    let ideal = available / count as f32;
    let tab_w = if ideal >= 48.0 { ideal.min(240.0) } else { ideal.max(20.0) };
    let mut tabs = Vec::with_capacity(count);
    let mut x = left;
    for _ in 0..count {
        let rect = Rect { x, y: 6.0, w: (tab_w - 4.0).max(16.0), h: 35.0 };
        let close = Rect {
            x: rect.right() - 8.0 - 16.0,
            y: rect.y + (rect.h - 16.0) / 2.0,
            w: 16.0,
            h: 16.0,
        };
        tabs.push(TabGeom {
            title_left: rect.x + 12.0 + 16.0 + 8.0,
            title_right: close.x - 6.0,
            rect,
            close,
        });
        x += tab_w;
    }
    let plus = Rect { x: x + 2.0, y: 7.0, w: 28.0, h: 28.0 };
    let search = Rect { x: plus.right() + 4.0, y: 7.0, w: 28.0, h: 28.0 };
    (tabs, plus, search)
}

fn toolbar(width: f32) -> Tools {
    let y = TAB_H + (TOOLBAR_H - BTN) / 2.0;
    let mut x = 6.0;
    let next = |x: &mut f32| {
        let rect = Rect { x: *x, y, w: BTN, h: BTN };
        *x += BTN + 4.0;
        rect
    };
    let back = next(&mut x);
    let forward = next(&mut x);
    let reload = next(&mut x);
    let home = next(&mut x);
    x += 5.0;
    let menu = Rect { x: width - 6.0 - BTN, y, w: BTN, h: BTN };
    let profile = Rect { x: menu.x - 4.0 - BTN, y, w: BTN, h: BTN };
    let extensions = Rect { x: profile.x - 4.0 - BTN, y, w: BTN, h: BTN };
    let address = Rect {
        x,
        y,
        w: (extensions.x - 8.0 - x).max(40.0),
        h: BTN,
    };
    let star = Rect { x: address.right() - BTN - 2.0, y, w: BTN, h: BTN };
    Tools { back, forward, reload, home, address, star, extensions, profile, menu }
}

fn bookmark_rects(state: &State<'_>) -> Vec<Rect> {
    let y = TAB_H + TOOLBAR_H + 3.0;
    let mut x = 8.0;
    let mut rects = Vec::new();
    for (title, _) in &state.bookmarks {
        let text = text_width(state.fonts, title, 13.0);
        let w = (text + 40.0).clamp(72.0, 180.0);
        if x + w > state.width - 8.0 {
            break;
        }
        rects.push(Rect { x, y, w, h: 28.0 });
        x += w + 4.0;
    }
    rects
}

fn active_tab(rect: Rect) -> Vec<(f32, f32)> {
    let radius = 8.0;
    let scoop = 10.0;
    let bottom = rect.bottom();
    let right = rect.right();
    let mut points = Vec::new();
    points.extend(arc(rect.x, bottom, scoop, std::f32::consts::PI, std::f32::consts::PI * 1.5, 6));
    points.push((rect.x, rect.y + radius));
    points.extend(arc(rect.x + radius, rect.y + radius, radius, std::f32::consts::PI, std::f32::consts::PI * 1.5, 6));
    points.push((right - radius, rect.y));
    points.extend(arc(right - radius, rect.y + radius, radius, -std::f32::consts::FRAC_PI_2, 0.0, 6));
    points.push((right, bottom - scoop));
    points.extend(arc(right, bottom, scoop, -std::f32::consts::FRAC_PI_2, 0.0, 6));
    points
}

fn arc(cx: f32, cy: f32, radius: f32, start: f32, end: f32, steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|step| {
            let t = start + (end - start) * (step as f32 / steps as f32);
            (cx + radius * t.cos(), cy + radius * t.sin())
        })
        .collect()
}

fn caption_at(width: f32, x: f32) -> Option<Hit> {
    if x >= width - CAPTION_W {
        Some(Hit::Close)
    } else if x >= width - CAPTION_W * 2.0 {
        Some(Hit::Maximize)
    } else if x >= width - CAPTION_W * 3.0 {
        Some(Hit::Minimize)
    } else {
        None
    }
}

fn paint_captions(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let specs = [
        (state.width - CAPTION_W * 3.0, Kind::Minimize, Hit::Minimize),
        (state.width - CAPTION_W * 2.0, Kind::Maximize, Hit::Maximize),
        (state.width - CAPTION_W, Kind::Close, Hit::Close),
    ];
    for (x, kind, which) in specs {
        let rect = Rect { x, y: 0.0, w: CAPTION_W, h: TAB_H };
        let over = rect.contains(state.pointer.0, state.pointer.1);
        if over && which == Hit::Close {
            fill_rect(pixels, width, height, x, 0.0, CAPTION_W, TAB_H, CLOSE_HOVER);
        } else if over {
            fill_rect(pixels, width, height, x, 0.0, CAPTION_W, TAB_H, HOVER);
        }
        let color = if over && which == Hit::Close { TOOLBAR } else { ICON };
        icons::blit(pixels, width, height, x + (CAPTION_W - 16.0) / 2.0, (TAB_H - 16.0) / 2.0, 16.0, kind, color);
    }
}

fn tool_icon(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, rect: Rect, kind: Kind, enabled: bool, label: &str) {
    button_disc(pixels, width, height, rect, state, enabled);
    icons::blit(
        pixels,
        width,
        height,
        rect.x + (rect.w - 18.0) / 2.0,
        rect.y + (rect.h - 18.0) / 2.0,
        18.0,
        kind,
        if enabled { ICON } else { ICON_OFF },
    );
    if enabled {
        tip(pixels, width, height, state, rect, label);
    }
}

fn button_disc(pixels: &mut [u32], width: u32, height: u32, rect: Rect, state: &State<'_>, enabled: bool) {
    if !enabled || !rect.contains(state.pointer.0, state.pointer.1) {
        return;
    }
    let alpha = if state.pressed { 36 } else { 18 };
    fill_circle_alpha(pixels, width, height, rect.x + rect.w / 2.0, rect.y + rect.h / 2.0, 16.0, ICON, alpha);
}

fn paint_profile(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, rect: Rect) {
    button_disc(pixels, width, height, rect, state, true);
    let cx = rect.x + rect.w / 2.0;
    let cy = rect.y + rect.h / 2.0;
    fill_circle(pixels, width, height, cx, cy, 12.0, PROFILE);
    let width_m = text_width(state.fonts, "M", 13.0);
    draw_text(pixels, width, height, state.fonts, cx - width_m / 2.0, cy + 5.0, 13.0, TOOLBAR, "M");
}

fn paint_bookmarks(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    if state.bookmarks.is_empty() {
        draw_text(
            pixels,
            width,
            height,
            state.fonts,
            16.0,
            TAB_H + TOOLBAR_H + 22.0,
            13.0,
            MUTED,
            "Bookmarks",
        );
        return;
    }
    for ((title, _), rect) in state.bookmarks.iter().zip(bookmark_rects(state)) {
        let over = rect.contains(state.pointer.0, state.pointer.1);
        fill_round(pixels, width, height, rect.x, rect.y, rect.w, rect.h, 14.0, if over { CHIP } else { TOOLBAR });
        icons::blit(pixels, width, height, rect.x + 8.0, rect.y + 6.0, 16.0, Kind::Globe, ICON_OFF);
        draw_text_fade(
            pixels,
            width,
            height,
            state.fonts,
            rect.x + 28.0,
            rect.y + 19.0,
            13.0,
            INK,
            title,
            rect.x + 28.0,
            rect.right() - 8.0,
            12.0,
        );
    }
}

fn paint_ntp(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let top = content_chrome(state);
    fill_rect(pixels, width, height, 0.0, top, state.width, state.height - top, TOOLBAR);
    let cx = state.width / 2.0;
    let cy = top + 150.0;
    fill_circle(pixels, width, height, cx, cy, 36.0, PROFILE);
    let mark = text_width(state.fonts, "M", 28.0);
    draw_text(pixels, width, height, state.fonts, cx - mark / 2.0, cy + 10.0, 28.0, TOOLBAR, "M");
    let word = text_width(state.fonts, "Mithu", 36.0);
    draw_text(pixels, width, height, state.fonts, cx - word / 2.0, cy + 86.0, 36.0, INK, "Mithu");
    let sub = "A small browser";
    let sub_w = text_width(state.fonts, sub, 14.0);
    draw_text(pixels, width, height, state.fonts, cx - sub_w / 2.0, cy + 116.0, 14.0, MUTED, sub);
}

fn paint_find(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let x = state.width - 280.0;
    let y = content_chrome(state) + 8.0;
    fill_round(pixels, width, height, x, y, 264.0, 36.0, 8.0, TOOLBAR);
    stroke_round(pixels, width, height, x, y, 264.0, 36.0, 8.0, LINE);
    let text = if state.find_query.is_empty() { "Find" } else { state.find_query.as_str() };
    let color = if state.find_query.is_empty() { ICON_OFF } else { INK };
    draw_text(pixels, width, height, state.fonts, x + 12.0, y + 23.0, 13.0, color, text);
}

fn paint_suggestions(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, address: Rect) {
    let h = state.suggestions.len() as f32 * 32.0 + 8.0;
    let y = address.bottom() + 4.0;
    shadow(pixels, width, height, address.x, y, address.w, h);
    fill_round(pixels, width, height, address.x, y, address.w, h, 12.0, TOOLBAR);
    for (index, suggestion) in state.suggestions.iter().enumerate() {
        let row = Rect { x: address.x + 4.0, y: y + 4.0 + index as f32 * 32.0, w: address.w - 8.0, h: 32.0 };
        if state.suggestion == Some(index) || row.contains(state.pointer.0, state.pointer.1) {
            fill_round(pixels, width, height, row.x, row.y, row.w, row.h, 8.0, CHIP);
        }
        draw_text_fade(pixels, width, height, state.fonts, row.x + 12.0, row.y + 21.0, 13.0, INK, suggestion, row.x + 12.0, row.right() - 12.0, 16.0);
    }
}

fn paint_tab_search(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, anchor: Rect) {
    let h = 12.0 + state.tabs.len().max(1) as f32 * 32.0;
    let x = anchor.x.min(state.width - 288.0).max(8.0);
    let y = anchor.bottom() + 6.0;
    shadow(pixels, width, height, x, y, 280.0, h);
    fill_round(pixels, width, height, x, y, 280.0, h, 12.0, TOOLBAR);
    for (index, tab) in state.tabs.iter().enumerate() {
        let row = Rect { x: x + 4.0, y: y + 6.0 + index as f32 * 32.0, w: 272.0, h: 32.0 };
        if index == state.active || row.contains(state.pointer.0, state.pointer.1) {
            fill_round(pixels, width, height, row.x, row.y, row.w, row.h, 8.0, CHIP);
        }
        draw_text_fade(pixels, width, height, state.fonts, row.x + 12.0, row.y + 21.0, 13.0, INK, &tab.title, row.x + 12.0, row.right() - 12.0, 16.0);
    }
}

fn extensions_rect(state: &State<'_>) -> Rect {
    let tools = toolbar(state.width);
    let w = 240.0;
    Rect {
        x: (tools.extensions.x - 80.0).min(state.width - w - 8.0).max(8.0),
        y: tools.extensions.bottom() + 6.0,
        w,
        h: 72.0,
    }
}

fn profile_rect(state: &State<'_>) -> Rect {
    let tools = toolbar(state.width);
    let w = 220.0;
    Rect {
        x: (tools.profile.x - 80.0).min(state.width - w - 8.0).max(8.0),
        y: tools.profile.bottom() + 6.0,
        w,
        h: 88.0,
    }
}

fn paint_extensions(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, _anchor: Rect) {
    let rect = extensions_rect(state);
    shadow(pixels, width, height, rect.x, rect.y, rect.w, rect.h);
    fill_round(pixels, width, height, rect.x, rect.y, rect.w, rect.h, 12.0, TOOLBAR);
    draw_text(pixels, width, height, state.fonts, rect.x + 16.0, rect.y + 28.0, 14.0, INK, "Extensions");
    draw_text(pixels, width, height, state.fonts, rect.x + 16.0, rect.y + 50.0, 12.0, MUTED, "No extensions installed.");
}

fn paint_profile_card(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, _anchor: Rect) {
    let rect = profile_rect(state);
    let x = rect.x;
    let y = rect.y;
    let w = rect.w;
    shadow(pixels, width, height, x, y, w, 88.0);
    fill_round(pixels, width, height, x, y, w, 88.0, 12.0, TOOLBAR);
    fill_circle(pixels, width, height, x + 28.0, y + 36.0, 16.0, PROFILE);
    draw_text(pixels, width, height, state.fonts, x + 22.0, y + 41.0, 14.0, TOOLBAR, "M");
    draw_text(pixels, width, height, state.fonts, x + 56.0, y + 34.0, 15.0, INK, "Mithu");
    draw_text(pixels, width, height, state.fonts, x + 56.0, y + 56.0, 12.0, MUTED, "Not signed in");
}

fn paint_menu(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, popup: Popup) {
    let rect = menu_rect(state, popup);
    shadow(pixels, width, height, rect.x, rect.y, rect.w, rect.h);
    fill_round(pixels, width, height, rect.x, rect.y, rect.w, rect.h, 12.0, TOOLBAR);
    for (index, label) in state.menu_labels.iter().enumerate() {
        let row = Rect { x: rect.x + 4.0, y: rect.y + 8.0 + index as f32 * 32.0, w: rect.w - 8.0, h: 32.0 };
        if row.contains(state.pointer.0, state.pointer.1) {
            fill_round(pixels, width, height, row.x, row.y, row.w, row.h, 8.0, CHIP);
        }
        let kind = menu_icon(label);
        icons::blit(pixels, width, height, row.x + 8.0, row.y + 8.0, 16.0, kind, ICON);
        draw_text(pixels, width, height, state.fonts, row.x + 32.0, row.y + 21.0, 13.0, INK, label);
    }
}

fn menu_icon(label: &str) -> Kind {
    match label {
        "New tab" => Kind::Plus,
        "History" => Kind::History,
        "Downloads" => Kind::Download,
        "Settings" => Kind::Settings,
        "Bookmark this page" => Kind::Star,
        "Back" => Kind::Back,
        "Reload" => Kind::Reload,
        _ => Kind::Globe,
    }
}

fn menu_rect(state: &State<'_>, popup: Popup) -> Rect {
    let h = 16.0 + state.menu_labels.len().max(1) as f32 * 32.0;
    let w = 240.0;
    match popup {
        Popup::App => {
            let tools = toolbar(state.width);
            Rect {
                x: (tools.menu.right() - w).max(8.0),
                y: tools.menu.bottom() + 4.0,
                w,
                h,
            }
        }
        Popup::Context => Rect {
            x: state.pointer.0.min(state.width - w - 8.0).max(8.0),
            y: state.pointer.1.min(state.height - h - 8.0).max(TAB_H),
            w,
            h,
        },
    }
}

fn paint_status(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let text = if !state.link_status.is_empty() {
        state.link_status.as_str()
    } else if state.loading {
        "Loading…"
    } else {
        return;
    };
    let text_w = text_width(state.fonts, text, 12.0).min(state.width * 0.6);
    let bubble = Rect { x: 8.0, y: state.height - 32.0, w: text_w + 20.0, h: 24.0 };
    fill_round(pixels, width, height, bubble.x, bubble.y, bubble.w, bubble.h, 4.0, BUBBLE);
    stroke_round(pixels, width, height, bubble.x, bubble.y, bubble.w, bubble.h, 4.0, LINE);
    draw_text_fade(
        pixels,
        width,
        height,
        state.fonts,
        bubble.x + 10.0,
        bubble.y + 16.0,
        12.0,
        MUTED,
        text,
        bubble.x + 10.0,
        bubble.right() - 8.0,
        14.0,
    );
}

fn tip(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, anchor: Rect, label: &str) {
    if state.pressed || !anchor.contains(state.pointer.0, state.pointer.1) {
        return;
    }
    let text_w = text_width(state.fonts, label, 12.0);
    let mut x = anchor.x + anchor.w / 2.0 - (text_w + 16.0) / 2.0;
    let y = anchor.bottom() + 6.0;
    x = x.clamp(4.0, (state.width - text_w - 20.0).max(4.0));
    fill_round(pixels, width, height, x, y, text_w + 16.0, 22.0, 4.0, rgb(60, 64, 67));
    draw_text(pixels, width, height, state.fonts, x + 8.0, y + 15.0, 12.0, TOOLBAR, label);
}

fn omnibox_text(url: &str) -> &str {
    match url {
        "" | "about:blank" | "about:newtab" | "about:mithu" | "about:home" => "",
        other => other,
    }
}

fn shadow(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32) {
    fill_round(pixels, width, height, x + 1.0, y + 2.0, w, h, 12.0, rgb(230, 230, 230));
}

fn blit_favicon(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, fw: u32, fh: u32, bytes: &[u8]) {
    if fw == 0 || fh == 0 {
        return;
    }
    for row in 0..16 {
        for col in 0..16 {
            let sx = (col * fw / 16).min(fw - 1);
            let sy = (row * fh / 16).min(fh - 1);
            let index = ((sy * fw + sx) * 4) as usize;
            if index + 3 >= bytes.len() {
                return;
            }
            let alpha = bytes[index + 3];
            if alpha == 0 {
                continue;
            }
            blend(
                pixels,
                width,
                height,
                x as i32 + col as i32,
                y as i32 + row as i32,
                rgb(bytes[index] as u32, bytes[index + 1] as u32, bytes[index + 2] as u32),
                alpha,
            );
        }
    }
}

fn draw_text(pixels: &mut [u32], width: u32, height: u32, fonts: &FontBook, x: f32, baseline: f32, size: f32, color: u32, text: &str) {
    draw_text_fade(pixels, width, height, fonts, x, baseline, size, color, text, f32::MIN, f32::MAX, 0.0);
}

fn draw_text_fade(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    fonts: &FontBook,
    x: f32,
    baseline: f32,
    size: f32,
    color: u32,
    text: &str,
    clip_left: f32,
    clip_right: f32,
    fade: f32,
) {
    let glyphs = fonts.shape(text, size, false, false, "");
    let mut pen = x;
    for glyph in glyphs {
        if pen >= clip_right {
            break;
        }
        let fade_alpha = if fade <= 0.0 || pen <= clip_right - fade {
            255u8
        } else if pen >= clip_right {
            0
        } else {
            (((clip_right - pen) / fade) * 255.0) as u8
        };
        blit_glyph(pixels, width, height, pen, baseline, &glyph, color, clip_left, clip_right, fade_alpha);
        pen += glyph.advance;
    }
}

fn text_width(fonts: &FontBook, text: &str, size: f32) -> f32 {
    fonts.shape(text, size, false, false, "").iter().map(|glyph| glyph.advance).sum()
}

fn blit_glyph(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    pen: f32,
    baseline: f32,
    glyph: &Glyph,
    color: u32,
    clip_left: f32,
    clip_right: f32,
    fade_alpha: u8,
) {
    if fade_alpha == 0 {
        return;
    }
    let left = (pen + glyph.x_offset).round() as i32 + glyph.xmin;
    let top = glyph_top(baseline, glyph);
    for row in 0..glyph.height {
        for col in 0..glyph.width {
            let px = left + col as i32;
            let py = top + row as i32;
            if (px as f32) < clip_left || (px as f32) >= clip_right {
                continue;
            }
            let coverage = glyph.coverage[row * glyph.width + col] as u32 * fade_alpha as u32 / 255;
            if coverage == 0 {
                continue;
            }
            blend(pixels, width, height, px, py, color, coverage as u8);
        }
    }
}

fn fill_rect(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32, color: u32) {
    let x0 = x.round().max(0.0) as u32;
    let y0 = y.round().max(0.0) as u32;
    let x1 = (x + w).round().clamp(0.0, width as f32) as u32;
    let y1 = (y + h).round().clamp(0.0, height as f32) as u32;
    for row in y0..y1 {
        for col in x0..x1 {
            pixels[(row * width + col) as usize] = color;
        }
    }
}

fn fill_round(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32) {
    let radius = radius.max(0.0).min(w / 2.0).min(h / 2.0);
    let x0 = x.floor().max(0.0) as i32;
    let y0 = y.floor().max(0.0) as i32;
    let x1 = (x + w).ceil().min(width as f32) as i32;
    let y1 = (y + h).ceil().min(height as f32) as i32;
    for py in y0..y1 {
        for px in x0..x1 {
            if inside_round(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, radius) {
                put(pixels, width, height, px, py, color);
            }
        }
    }
}

fn stroke_round(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32) {
    let radius = radius.max(0.0);
    let x0 = x.floor().max(0.0) as i32;
    let y0 = y.floor().max(0.0) as i32;
    let x1 = (x + w).ceil().min(width as f32) as i32;
    let y1 = (y + h).ceil().min(height as f32) as i32;
    for py in y0..y1 {
        for px in x0..x1 {
            let sx = px as f32 + 0.5;
            let sy = py as f32 + 0.5;
            let on = inside_round(sx, sy, x, y, w, h, radius)
                && !inside_round(sx, sy, x + 1.5, y + 1.5, w - 3.0, h - 3.0, (radius - 1.5).max(0.0));
            if on {
                put(pixels, width, height, px, py, color);
            }
        }
    }
}

fn inside_round(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, radius: f32) -> bool {
    if px < x || py < y || px >= x + w || py >= y + h {
        return false;
    }
    let dx = if px < x + radius {
        x + radius - px
    } else if px > x + w - radius {
        px - (x + w - radius)
    } else {
        0.0
    };
    let dy = if py < y + radius {
        y + radius - py
    } else if py > y + h - radius {
        py - (y + h - radius)
    } else {
        0.0
    };
    dx * dx + dy * dy <= radius * radius
}

fn fill_circle(pixels: &mut [u32], width: u32, height: u32, cx: f32, cy: f32, radius: f32, color: u32) {
    fill_circle_alpha(pixels, width, height, cx, cy, radius, color, 255);
}

fn fill_circle_alpha(pixels: &mut [u32], width: u32, height: u32, cx: f32, cy: f32, radius: f32, color: u32, alpha: u8) {
    let x0 = (cx - radius).floor().max(0.0) as i32;
    let y0 = (cy - radius).floor().max(0.0) as i32;
    let x1 = (cx + radius).ceil().min(width as f32) as i32;
    let y1 = (cy + radius).ceil().min(height as f32) as i32;
    for py in y0..y1 {
        for px in x0..x1 {
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= radius * radius {
                if alpha == 255 {
                    put(pixels, width, height, px, py, color);
                } else {
                    blend(pixels, width, height, px, py, color, alpha);
                }
            }
        }
    }
}

fn fill_polygon(pixels: &mut [u32], width: u32, height: u32, points: &[(f32, f32)], color: u32) {
    if points.len() < 3 {
        return;
    }
    let min_y = points.iter().map(|point| point.1).fold(f32::MAX, f32::min);
    let max_y = points.iter().map(|point| point.1).fold(f32::MIN, f32::max);
    let y0 = min_y.floor().max(0.0) as i32;
    let y1 = max_y.ceil().min(height as f32) as i32;
    for y in y0..y1 {
        let scan = y as f32 + 0.5;
        let mut crossings = Vec::new();
        for index in 0..points.len() {
            let (x1, y1) = points[index];
            let (x2, y2) = points[(index + 1) % points.len()];
            if (y1 <= scan && y2 > scan) || (y2 <= scan && y1 > scan) {
                let t = (scan - y1) / (y2 - y1);
                crossings.push(x1 + t * (x2 - x1));
            }
        }
        crossings.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        let mut index = 0;
        while index + 1 < crossings.len() {
            let x0 = crossings[index].floor().max(0.0) as i32;
            let x1 = crossings[index + 1].ceil().min(width as f32) as i32;
            for x in x0..x1 {
                put(pixels, width, height, x, y, color);
            }
            index += 2;
        }
    }
}

fn put(pixels: &mut [u32], width: u32, height: u32, x: i32, y: i32, color: u32) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    pixels[(y as u32 * width + x as u32) as usize] = color;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_sits_between_reload_and_the_omnibox() {
        let tools = toolbar(1280.0);
        assert!(tools.home.x > tools.reload.x);
        assert!(tools.address.x > tools.home.x);
        assert!(tools.star.x > tools.address.x);
        assert!(tools.star.right() <= tools.address.right() + 1.0);
        assert!(tools.extensions.x >= tools.address.right() - 1.0);
        assert!(tools.menu.right() <= 1280.0);
    }

    #[test]
    fn tab_title_is_clipped_inside_the_tab_and_captions_stay_clear() {
        let (tabs, _plus, search) = tab_layout(1280.0, 1);
        let tab = &tabs[0];
        assert!(tab.title_right < tab.close.x);
        assert!(tab.close.right() <= tab.rect.right());
        assert!(tab.rect.w <= 240.0);
        assert!(search.right() < 1280.0 - CAPTION_W * 3.0);
        let (many, plus, _) = tab_layout(1280.0, 30);
        assert!(many.last().unwrap().rect.right() <= plus.x + 1.0);
        assert!(plus.right() < 1280.0 - CAPTION_W * 3.0);
    }
}
