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
const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    (r << 16) | (g << 8) | b
}

thread_local! {
    static PAINT_SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
    static PAINT_DARK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn begin_paint(state: &State<'_>) {
    PAINT_SCALE.set(unit(state.scale));
    PAINT_DARK.set(state.dark);
}

fn paint_scale() -> f32 {
    PAINT_SCALE.get()
}

fn dip(logical: f32) -> f32 {
    snap(logical, paint_scale())
}

fn tone(light: u32, dark: u32) -> u32 {
    if PAINT_DARK.get() { dark } else { light }
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
    pub submenu_at: Option<usize>,
    pub submenu_labels: Vec<String>,
    pub tab_search: bool,
    pub extensions: bool,
    pub profile: bool,
    pub suggestions: Vec<String>,
    pub suggestion: Option<usize>,
    pub find_open: bool,
    pub find_query: String,
    pub fonts: &'a FontBook,
    /// Device pixels per logical pixel. 1.0, 1.25, 1.5, and 2.0 are the ones we snap.
    pub scale: f32,
    pub dark: bool,
    pub show_home: bool,
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
    SubmenuItem(usize),
    Zoom(i8),
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
    begin_paint(state);
    let w = state.width;
    let scale = unit(state.scale);
    let tab_h = snap(TAB_H, scale);
    let tool_h = snap(TOOLBAR_H, scale);
    let book_h = snap(BOOKMARK_BAR_H, scale);
    let bar = content_chrome(state);
    fill_rect(pixels, width, height, 0.0, 0.0, w, tab_h, tone(TAB_STRIP, rgb(31, 32, 32)));
    fill_rect(pixels, width, height, 0.0, tab_h, w, tool_h, tone(TOOLBAR, rgb(60, 60, 60)));
    if state.bookmarks_open {
        fill_rect(pixels, width, height, 0.0, tab_h + tool_h, w, book_h, tone(TOOLBAR, rgb(60, 60, 60)));
    }
    fill_rect(pixels, width, height, 0.0, bar - 1.0, w, 1.0, tone(LINE, rgb(60, 64, 67)));
    let app_mark = snap(22.0, scale).max(1.0);
    crate::brand::blit(
        pixels,
        width,
        height,
        snap(10.0, scale),
        ((tab_h - app_mark) / 2.0).round(),
        app_mark,
        app_mark,
        crate::brand::mark(),
    );

    let (tabs, plus, search) = tab_layout(w, state.tabs.len().max(1), state.scale);
    for (index, geom) in tabs.iter().enumerate() {
        let active = index == state.active;
        let hovered = geom.rect.contains(state.pointer.0, state.pointer.1);
        if active {
            fill_polygon(pixels, width, height, &active_tab(geom.rect), TOOLBAR);
        } else if hovered {
            fill_round(pixels, width, height, geom.rect.x, geom.rect.y, geom.rect.w, geom.rect.h - dip(4.0), dip(8.0), PLUS);
        } else if index + 1 != state.active && index != state.active.saturating_add(1) {
            let sep_x = geom.rect.right() + 1.0;
            fill_rect(pixels, width, height, sep_x, dip(14.0), 1.0, dip(16.0), rgb(160, 180, 210));
        }
        let title = state.tabs.get(index).map(|tab| tab.title.as_str()).unwrap_or("New Tab");
        let icon = dip(16.0).max(1.0);
        if let Some((fw, fh, bytes)) = state.tabs.get(index).and_then(|tab| tab.favicon.as_ref()) {
            blit_favicon(pixels, width, height, geom.rect.x + dip(12.0), geom.rect.y + (geom.rect.h - icon) / 2.0, icon, *fw, *fh, bytes);
        } else {
            crate::brand::blit(
                pixels,
                width,
                height,
                geom.rect.x + dip(10.0),
                geom.rect.y + (geom.rect.h - icon) / 2.0,
                icon,
                icon,
                crate::brand::mark(),
            );
        }
        if geom.title_right - geom.title_left > dip(8.0) {
            draw_text_fade(
                pixels,
                width,
                height,
                state.fonts,
                geom.title_left,
                geom.rect.y + dip(22.0),
                12.0,
                INK,
                title,
                geom.title_left,
                geom.title_right,
                dip(16.0),
            );
        }
        let show_close = active || geom.rect.contains(state.pointer.0, state.pointer.1);
        if show_close && geom.rect.w >= dip(36.0) {
            let over = geom.close.contains(state.pointer.0, state.pointer.1);
            if over {
                fill_circle(
                    pixels,
                    width,
                    height,
                    geom.close.x + geom.close.w / 2.0,
                    geom.close.y + geom.close.h / 2.0,
                    dip(10.0),
                    CLOSE_HOVER,
                );
            }
            icons::blit(
                pixels,
                width,
                height,
                geom.close.x,
                geom.close.y,
                geom.close.w,
                Kind::Close,
                if over { TOOLBAR } else { ICON },
            );
        }
    }
    button_disc(pixels, width, height, plus, state, true);
    centered_icon(pixels, width, height, plus, Kind::Plus, ICON);
    button_disc(pixels, width, height, search, state, true);
    centered_icon(pixels, width, height, search, Kind::Search, ICON);
    paint_captions(pixels, width, height, state);
    tip(pixels, width, height, state, plus, "New tab");
    tip(pixels, width, height, state, search, "Search tabs");

    let tools = toolbar(w, state.scale, state.show_home);
    tool_icon(pixels, width, height, state, tools.back, Kind::Back, state.can_back, "Back");
    tool_icon(pixels, width, height, state, tools.forward, Kind::Forward, state.can_forward, "Forward");
    let reload_kind = if state.loading { Kind::Stop } else { Kind::Reload };
    let reload_tip = if state.loading { "Stop" } else { "Reload" };
    tool_icon(pixels, width, height, state, tools.reload, reload_kind, true, reload_tip);
    if state.show_home && tools.home.w > 0.0 {
        tool_icon(pixels, width, height, state, tools.home, Kind::Home, true, "Home");
    }

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
    let mut text_left = tools.address.x + dip(14.0);
    let baseline = tools.address.y + dip(22.0);
    match state.secure {
        Secure::Https => {
            let icon = dip(16.0).max(1.0);
            icons::blit(pixels, width, height, text_left, tools.address.y + (tools.address.h - icon) / 2.0, icon, Kind::Lock, ICON);
            text_left += icon + dip(8.0);
        }
        Secure::Http => {
            let label = "Not secure";
            draw_text(pixels, width, height, state.fonts, text_left, baseline, 13.0, NOT_SECURE, label);
            text_left += text_width(state.fonts, label, 13.0) + dip(10.0);
        }
        Secure::Internal => {
            let icon = dip(16.0).max(1.0);
            crate::brand::blit(
                pixels,
                width,
                height,
                text_left,
                tools.address.y + (tools.address.h - icon) / 2.0,
                icon,
                icon,
                crate::brand::mark(),
            );
            text_left += icon + dip(8.0);
        }
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
            baseline,
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
            baseline,
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
                fill_rect(pixels, width, height, caret, tools.address.y + dip(8.0), 1.0, dip(18.0), INK);
            }
        }
    }
    button_disc(pixels, width, height, tools.star, state, true);
    centered_icon(
        pixels,
        width,
        height,
        tools.star,
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
    begin_paint(state);
    let tools = toolbar(state.width, state.scale, state.show_home);
    if state.menu.is_some() {
        if let Some(hit) = menu_hit(state, x, y) {
            return hit;
        }
    }
    if state.tab_search {
        let (_, _, search) = tab_layout(state.width, state.tabs.len().max(1), state.scale);
        let rect = tab_search_rect(state, search);
        if rect.contains(x, y) {
            let index = ((y - rect.y - dip(6.0)) / dip(32.0)).floor() as isize;
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
            y: tools.address.bottom() + dip(4.0),
            w: tools.address.w,
            h: state.suggestions.len() as f32 * dip(32.0) + dip(8.0),
        };
        if rect.contains(x, y) {
            let index = ((y - rect.y - dip(4.0)) / dip(32.0)).floor() as isize;
            if index >= 0 && (index as usize) < state.suggestions.len() {
                return Hit::Suggestion(index as usize);
            }
            return Hit::Panel;
        }
    }
    let scale = unit(state.scale);
    let tab_h = snap(TAB_H, scale);
    let tool_h = snap(TOOLBAR_H, scale);
    let book_h = snap(BOOKMARK_BAR_H, scale);
    if y < tab_h {
        if let Some(caption) = caption_at(state.width, x, state.scale) {
            return caption;
        }
        let (tabs, plus, search) = tab_layout(state.width, state.tabs.len().max(1), state.scale);
        if plus.contains(x, y) {
            return Hit::NewTab;
        }
        if search.contains(x, y) {
            return Hit::TabSearch;
        }
        for (index, geom) in tabs.iter().enumerate() {
            if geom.close.contains(x, y) && geom.rect.w >= dip(36.0) {
                return Hit::CloseTab(index);
            }
            if geom.rect.contains(x, y) {
                return Hit::Tab(index);
            }
        }
        return Hit::Drag;
    }
    if y < tab_h + tool_h {
        if tools.back.contains(x, y) && state.can_back {
            return Hit::Back;
        }
        if tools.forward.contains(x, y) && state.can_forward {
            return Hit::Forward;
        }
        if tools.reload.contains(x, y) {
            return Hit::Reload;
        }
        if state.show_home && tools.home.w > 0.0 && tools.home.contains(x, y) {
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
    if state.bookmarks_open && y < tab_h + tool_h + book_h {
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

pub fn chrome_height(bookmarks_open: bool, scale: f32) -> f32 {
    let scale = unit(scale);
    let mut top = snap(TAB_H, scale) + snap(TOOLBAR_H, scale);
    if bookmarks_open {
        top += snap(BOOKMARK_BAR_H, scale);
    }
    top
}

pub fn content_chrome(state: &State<'_>) -> f32 {
    chrome_height(state.bookmarks_open, state.scale)
}

fn unit(scale: f32) -> f32 {
    if scale.is_finite() && scale >= 1.0 { scale } else { 1.0 }
}

fn snap(logical: f32, scale: f32) -> f32 {
    (logical * unit(scale)).round()
}

fn box_px(x: f32, y: f32, w: f32, h: f32, scale: f32) -> Rect {
    let x0 = snap(x, scale);
    let y0 = snap(y, scale);
    let x1 = snap(x + w, scale).max(x0 + 1.0);
    let y1 = snap(y + h, scale).max(y0 + 1.0);
    Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

fn tab_layout(width_px: f32, count: usize, scale: f32) -> (Vec<TabGeom>, Rect, Rect) {
    let scale = unit(scale);
    let width = width_px / scale;
    let count = count.max(1);
    let left = 40.0;
    let reserved = 8.0 + 28.0 + 4.0 + 28.0 + 12.0 + CAPTION_W * 3.0;
    let available = (width - left - reserved).max(16.0);
    let ideal = available / count as f32;
    let tab_w = if ideal >= 48.0 { ideal.min(240.0) } else { ideal.max(20.0) };
    let mut tabs = Vec::with_capacity(count);
    let mut x = left;
    let strip_bottom = TAB_H;
    for _ in 0..count {
        let rect = box_px(x, 6.0, (tab_w - 4.0).max(16.0), strip_bottom - 6.0, scale);
        let close = box_px(
            (x + (tab_w - 4.0).max(16.0)) - 8.0 - 16.0,
            6.0 + (strip_bottom - 6.0 - 16.0) / 2.0,
            16.0,
            16.0,
            scale,
        );
        let title_left = snap(x + 12.0 + 16.0 + 8.0, scale);
        let title_right = close.x - snap(6.0, scale);
        tabs.push(TabGeom { title_left, title_right, rect, close });
        x += tab_w;
    }
    let plus = box_px(x + 2.0, 7.0, 28.0, 28.0, scale);
    let search = box_px(x + 2.0 + 28.0 + 4.0, 7.0, 28.0, 28.0, scale);
    (tabs, plus, search)
}

fn toolbar(width_px: f32, scale: f32, show_home: bool) -> Tools {
    let scale = unit(scale);
    let width = width_px / scale;
    let y = TAB_H + (TOOLBAR_H - BTN) / 2.0;
    let mut x = 6.0;
    let place = |x: &mut f32| {
        let rect = box_px(*x, y, BTN, BTN, scale);
        *x += BTN + 4.0;
        rect
    };
    let back = place(&mut x);
    let forward = place(&mut x);
    let reload = place(&mut x);
    let home = if show_home {
        place(&mut x)
    } else {
        Rect { x: snap(x, scale), y: snap(y, scale), w: 0.0, h: 0.0 }
    };
    x += 5.0;
    let menu = box_px(width - 6.0 - BTN, y, BTN, BTN, scale);
    let profile = box_px(width - 6.0 - BTN - 4.0 - BTN, y, BTN, BTN, scale);
    let extensions = box_px(width - 6.0 - BTN - 4.0 - BTN - 4.0 - BTN, y, BTN, BTN, scale);
    let address_w = (width - 6.0 - BTN - 4.0 - BTN - 4.0 - BTN - 8.0 - x).max(40.0);
    let address = box_px(x, y, address_w, BTN, scale);
    let star = box_px(x + address_w - BTN - 2.0, y, BTN, BTN, scale);
    Tools { back, forward, reload, home, address, star, extensions, profile, menu }
}

fn bookmark_rects(state: &State<'_>) -> Vec<Rect> {
    let scale = unit(state.scale);
    let y = snap(TAB_H, scale) + snap(TOOLBAR_H, scale) + snap(3.0, scale);
    let mut x = dip(8.0);
    let mut rects = Vec::new();
    for (title, _) in &state.bookmarks {
        let text = text_width(state.fonts, title, 13.0);
        let w = (text + dip(40.0)).clamp(dip(72.0), dip(180.0));
        if x + w > state.width - dip(8.0) {
            break;
        }
        rects.push(Rect { x, y, w, h: dip(28.0) });
        x += w + dip(4.0);
    }
    rects
}

fn active_tab(rect: Rect) -> Vec<(f32, f32)> {
    let radius = dip(8.0);
    let scoop = dip(10.0);
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

fn caption_at(width: f32, x: f32, scale: f32) -> Option<Hit> {
    let caption = snap(CAPTION_W, scale);
    if x >= width - caption {
        Some(Hit::Close)
    } else if x >= width - caption * 2.0 {
        Some(Hit::Maximize)
    } else if x >= width - caption * 3.0 {
        Some(Hit::Minimize)
    } else {
        None
    }
}

fn paint_captions(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let caption = snap(CAPTION_W, unit(state.scale));
    let tab_h = snap(TAB_H, unit(state.scale));
    let specs = [
        (state.width - caption * 3.0, Kind::Minimize, Hit::Minimize),
        (state.width - caption * 2.0, Kind::Maximize, Hit::Maximize),
        (state.width - caption, Kind::Close, Hit::Close),
    ];
    for (x, kind, which) in specs {
        let rect = Rect { x, y: 0.0, w: caption, h: tab_h };
        let over = rect.contains(state.pointer.0, state.pointer.1);
        if over && which == Hit::Close {
            fill_rect(pixels, width, height, x, 0.0, caption, tab_h, CLOSE_HOVER);
        } else if over {
            fill_rect(pixels, width, height, x, 0.0, caption, tab_h, tone(HOVER, rgb(48, 51, 56)));
        }
        let color = if over && which == Hit::Close { tone(TOOLBAR, rgb(32, 34, 38)) } else { tone(ICON, rgb(232, 234, 237)) };
        let icon = snap(16.0, unit(state.scale)).max(1.0);
        icons::blit(
            pixels,
            width,
            height,
            (x + (caption - icon) / 2.0).round(),
            ((tab_h - icon) / 2.0).round(),
            icon,
            kind,
            color,
        );
    }
}

fn centered_icon(pixels: &mut [u32], width: u32, height: u32, rect: Rect, kind: Kind, color: u32) {
    let icon = dip(16.0).max(1.0);
    icons::blit(
        pixels,
        width,
        height,
        (rect.x + (rect.w - icon) / 2.0).round(),
        (rect.y + (rect.h - icon) / 2.0).round(),
        icon,
        kind,
        color,
    );
}

fn tool_icon(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, rect: Rect, kind: Kind, enabled: bool, label: &str) {
    button_disc(pixels, width, height, rect, state, enabled);
    let icon = snap(16.0, unit(state.scale)).max(1.0);
    icons::blit(
        pixels,
        width,
        height,
        (rect.x + (rect.w - icon) / 2.0).round(),
        (rect.y + (rect.h - icon) / 2.0).round(),
        icon,
        kind,
        if enabled { tone(ICON, rgb(232, 234, 237)) } else { tone(ICON_OFF, rgb(154, 160, 166)) },
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
    fill_circle_alpha(pixels, width, height, rect.x + rect.w / 2.0, rect.y + rect.h / 2.0, rect.w * 0.46, ICON, alpha);
}

fn paint_profile(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, rect: Rect) {
    button_disc(pixels, width, height, rect, state, true);
    let cx = rect.x + rect.w / 2.0;
    let cy = rect.y + rect.h / 2.0;
    let icon = snap(22.0, unit(state.scale)).max(1.0);
    crate::brand::blit(pixels, width, height, cx - icon / 2.0, cy - icon / 2.0, icon, icon, crate::brand::mark());
}

fn paint_bookmarks(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    if state.bookmarks.is_empty() {
        draw_text(
            pixels,
            width,
            height,
            state.fonts,
            dip(16.0),
            snap(TAB_H, paint_scale()) + snap(TOOLBAR_H, paint_scale()) + dip(22.0),
            13.0,
            MUTED,
            "Bookmarks",
        );
        return;
    }
    for ((title, _), rect) in state.bookmarks.iter().zip(bookmark_rects(state)) {
        let over = rect.contains(state.pointer.0, state.pointer.1);
        fill_round(pixels, width, height, rect.x, rect.y, rect.w, rect.h, dip(14.0), if over { CHIP } else { TOOLBAR });
        let icon = dip(16.0).max(1.0);
        icons::blit(pixels, width, height, rect.x + dip(8.0), rect.y + (rect.h - icon) / 2.0, icon, Kind::Globe, ICON_OFF);
        draw_text_fade(
            pixels,
            width,
            height,
            state.fonts,
            rect.x + dip(28.0),
            rect.y + dip(19.0),
            13.0,
            INK,
            title,
            rect.x + dip(28.0),
            rect.right() - dip(8.0),
            dip(12.0),
        );
    }
}

fn paint_ntp(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let top = content_chrome(state);
    fill_rect(pixels, width, height, 0.0, top, state.width, state.height - top, if state.dark { rgb(31, 31, 31) } else { rgb(255, 255, 255) });
    let size = snap(220.0, unit(state.scale)).min(state.width * 0.46).max(1.0);
    let cx = state.width / 2.0;
    let y = top + snap(48.0, unit(state.scale));
    crate::brand::blit(pixels, width, height, cx - size / 2.0, y, size, size, crate::brand::logo());
}

fn paint_find(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>) {
    let bar_w = dip(264.0);
    let bar_h = dip(36.0);
    let x = state.width - dip(280.0);
    let y = content_chrome(state) + dip(8.0);
    fill_round(pixels, width, height, x, y, bar_w, bar_h, dip(8.0), TOOLBAR);
    stroke_round(pixels, width, height, x, y, bar_w, bar_h, dip(8.0), LINE);
    let text = if state.find_query.is_empty() { "Find" } else { state.find_query.as_str() };
    let color = if state.find_query.is_empty() { ICON_OFF } else { INK };
    draw_text(pixels, width, height, state.fonts, x + dip(12.0), y + dip(23.0), 13.0, color, text);
}

fn paint_suggestions(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, address: Rect) {
    let row_h = dip(32.0);
    let h = state.suggestions.len() as f32 * row_h + dip(8.0);
    let y = address.bottom() + dip(4.0);
    shadow(pixels, width, height, address.x, y, address.w, h);
    fill_round(pixels, width, height, address.x, y, address.w, h, dip(12.0), TOOLBAR);
    for (index, suggestion) in state.suggestions.iter().enumerate() {
        let row = Rect { x: address.x + dip(4.0), y: y + dip(4.0) + index as f32 * row_h, w: address.w - dip(8.0), h: row_h };
        if state.suggestion == Some(index) || row.contains(state.pointer.0, state.pointer.1) {
            fill_round(pixels, width, height, row.x, row.y, row.w, row.h, dip(8.0), CHIP);
        }
        draw_text_fade(pixels, width, height, state.fonts, row.x + dip(12.0), row.y + dip(21.0), 13.0, INK, suggestion, row.x + dip(12.0), row.right() - dip(12.0), dip(16.0));
    }
}

fn tab_search_rect(state: &State<'_>, anchor: Rect) -> Rect {
    let w = dip(280.0);
    let h = dip(12.0) + state.tabs.len().max(1) as f32 * dip(32.0);
    Rect {
        x: anchor.x.min(state.width - w - dip(8.0)).max(dip(8.0)),
        y: anchor.bottom() + dip(6.0),
        w,
        h,
    }
}

fn paint_tab_search(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, anchor: Rect) {
    let rect = tab_search_rect(state, anchor);
    let row_h = dip(32.0);
    shadow(pixels, width, height, rect.x, rect.y, rect.w, rect.h);
    fill_round(pixels, width, height, rect.x, rect.y, rect.w, rect.h, dip(12.0), TOOLBAR);
    for (index, tab) in state.tabs.iter().enumerate() {
        let row = Rect {
            x: rect.x + dip(4.0),
            y: rect.y + dip(6.0) + index as f32 * row_h,
            w: rect.w - dip(8.0),
            h: row_h,
        };
        if index == state.active || row.contains(state.pointer.0, state.pointer.1) {
            fill_round(pixels, width, height, row.x, row.y, row.w, row.h, dip(8.0), CHIP);
        }
        draw_text_fade(pixels, width, height, state.fonts, row.x + dip(12.0), row.y + dip(21.0), 13.0, INK, &tab.title, row.x + dip(12.0), row.right() - dip(12.0), dip(16.0));
    }
}

fn extensions_rect(state: &State<'_>) -> Rect {
    let tools = toolbar(state.width, state.scale, state.show_home);
    let w = dip(240.0);
    let h = dip(72.0);
    Rect {
        x: (tools.extensions.x - dip(80.0)).min(state.width - w - dip(8.0)).max(dip(8.0)),
        y: tools.extensions.bottom() + dip(6.0),
        w,
        h,
    }
}

fn profile_rect(state: &State<'_>) -> Rect {
    let tools = toolbar(state.width, state.scale, state.show_home);
    let w = dip(280.0);
    let h = dip(132.0);
    Rect {
        x: (tools.profile.x - dip(80.0)).min(state.width - w - dip(8.0)).max(dip(8.0)),
        y: tools.profile.bottom() + dip(6.0),
        w,
        h,
    }
}

fn paint_extensions(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, _anchor: Rect) {
    let rect = extensions_rect(state);
    shadow(pixels, width, height, rect.x, rect.y, rect.w, rect.h);
    fill_round(pixels, width, height, rect.x, rect.y, rect.w, rect.h, dip(12.0), TOOLBAR);
    draw_text(pixels, width, height, state.fonts, rect.x + dip(16.0), rect.y + dip(28.0), 14.0, INK, "Extensions");
    draw_text(pixels, width, height, state.fonts, rect.x + dip(16.0), rect.y + dip(50.0), 12.0, MUTED, "No extensions installed.");
}

fn paint_profile_card(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, _anchor: Rect) {
    let rect = profile_rect(state);
    let x = rect.x;
    let y = rect.y;
    let w = rect.w;
    shadow(pixels, width, height, x, y, w, rect.h);
    fill_round(pixels, width, height, x, y, w, rect.h, dip(12.0), TOOLBAR);
    let mark = dip(32.0).max(1.0);
    crate::brand::blit(pixels, width, height, x + dip(12.0), y + dip(16.0), mark, mark, crate::brand::mark());
    draw_text(pixels, width, height, state.fonts, x + dip(56.0), y + dip(28.0), 15.0, INK, "Mithu");
    draw_text(pixels, width, height, state.fonts, x + dip(56.0), y + dip(48.0), 12.0, MUTED, "Not signed in");
    draw_text(pixels, width, height, state.fonts, x + dip(16.0), y + dip(84.0), 13.0, INK, "Customize profile");
    draw_text(pixels, width, height, state.fonts, x + dip(16.0), y + dip(110.0), 13.0, INK, "Settings");
}

fn paint_menu(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, popup: Popup) {
    let rect = menu_rect(state, popup);
    shadow(pixels, width, height, rect.x, rect.y, rect.w, rect.h);
    fill_round(pixels, width, height, rect.x, rect.y, rect.w, rect.h, dip(12.0), TOOLBAR);
    let mut y = rect.y + dip(8.0);
    for (index, label) in state.menu_labels.iter().enumerate() {
        let row_h = if label.is_empty() { dip(9.0) } else { dip(32.0) };
        let row = Rect { x: rect.x + dip(4.0), y, w: rect.w - dip(8.0), h: row_h };
        if label.is_empty() {
            fill_rect(pixels, width, height, rect.x + dip(12.0), y + dip(4.0), rect.w - dip(24.0), 1.0, LINE);
        } else {
            let hot = row.contains(state.pointer.0, state.pointer.1) || state.submenu_at == Some(index);
            if hot {
                fill_round(pixels, width, height, row.x, row.y, row.w, row.h, dip(8.0), CHIP);
            }
            let (title, shortcut) = label.split_once('\t').unwrap_or((label.as_str(), ""));
            let kind = menu_icon(title.trim_end_matches('\u{203a}'));
            let icon = dip(16.0).max(1.0);
            icons::blit(pixels, width, height, row.x + dip(8.0), row.y + (row.h - icon) / 2.0, icon, kind, ICON);
            draw_text(pixels, width, height, state.fonts, row.x + dip(32.0), row.y + dip(21.0), 13.0, INK, title);
            if title == "Zoom" {
                paint_zoom_controls(pixels, width, height, state, row, shortcut);
            } else if !shortcut.is_empty() {
                let sw = text_width(state.fonts, shortcut, 12.0);
                draw_text(pixels, width, height, state.fonts, row.right() - dip(12.0) - sw, row.y + dip(21.0), 12.0, MUTED, shortcut);
            }
        }
        y += row_h;
    }
    if let Some(anchor) = state.submenu_at {
        if !state.submenu_labels.is_empty() {
            paint_submenu(pixels, width, height, state, rect, anchor);
        }
    }
}

fn paint_zoom_controls(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, row: Rect, percent: &str) {
    let buttons = ["−", percent, "+", "⛶"];
    let mut x = row.right() - dip(156.0);
    for label in buttons {
        let w = if label == percent { dip(52.0) } else { dip(28.0) };
        let cell = Rect { x, y: row.y + dip(2.0), w, h: row.h - dip(4.0) };
        if cell.contains(state.pointer.0, state.pointer.1) {
            fill_round(pixels, width, height, cell.x, cell.y, cell.w, cell.h, dip(6.0), PLUS);
        }
        let tw = text_width(state.fonts, label, 12.0);
        draw_text(pixels, width, height, state.fonts, cell.x + (cell.w - tw) / 2.0, cell.y + dip(18.0), 12.0, INK, label);
        x += w;
    }
}

fn paint_submenu(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, parent: Rect, anchor: usize) {
    let sub = submenu_rect(state, parent, anchor);
    shadow(pixels, width, height, sub.x, sub.y, sub.w, sub.h);
    fill_round(pixels, width, height, sub.x, sub.y, sub.w, sub.h, dip(12.0), TOOLBAR);
    let mut y = sub.y + dip(8.0);
    for label in &state.submenu_labels {
        let row_h = if label.is_empty() { dip(9.0) } else { dip(32.0) };
        let row = Rect { x: sub.x + dip(4.0), y, w: sub.w - dip(8.0), h: row_h };
        if label.is_empty() {
            fill_rect(pixels, width, height, sub.x + dip(12.0), y + dip(4.0), sub.w - dip(24.0), 1.0, LINE);
        } else {
            if row.contains(state.pointer.0, state.pointer.1) {
                fill_round(pixels, width, height, row.x, row.y, row.w, row.h, dip(8.0), CHIP);
            }
            let (title, shortcut) = label.split_once('\t').unwrap_or((label.as_str(), ""));
            draw_text(pixels, width, height, state.fonts, row.x + dip(12.0), row.y + dip(21.0), 13.0, INK, title);
            if !shortcut.is_empty() {
                let sw = text_width(state.fonts, shortcut, 12.0);
                draw_text(pixels, width, height, state.fonts, row.right() - dip(12.0) - sw, row.y + dip(21.0), 12.0, MUTED, shortcut);
            }
        }
        y += row_h;
    }
}

fn submenu_rect(state: &State<'_>, parent: Rect, anchor: usize) -> Rect {
    let w = dip(280.0);
    let h = menu_block_height(&state.submenu_labels);
    let y = parent.y + dip(8.0) + row_offset(&state.menu_labels, anchor);
    let x = if parent.x > w + dip(8.0) { parent.x - w - dip(4.0) } else { parent.right() + dip(4.0) };
    Rect { x, y, w, h }
}

fn menu_block_height(labels: &[String]) -> f32 {
    let mut h = dip(16.0);
    if labels.is_empty() {
        return h + dip(32.0);
    }
    for label in labels {
        h += if label.is_empty() { dip(9.0) } else { dip(32.0) };
    }
    h
}

fn menu_hit(state: &State<'_>, x: f32, y: f32) -> Option<Hit> {
    let rect = menu_rect(state, state.menu?);
    if let Some(anchor) = state.submenu_at {
        if !state.submenu_labels.is_empty() {
            let sub = submenu_rect(state, rect, anchor);
            if sub.contains(x, y) {
                return menu_index_at(&state.submenu_labels, y - sub.y).map(Hit::SubmenuItem).or(Some(Hit::Panel));
            }
        }
    }
    if !rect.contains(x, y) {
        return None;
    }
    let index = menu_index_at(&state.menu_labels, y - rect.y)?;
    let label = state.menu_labels.get(index).map(String::as_str).unwrap_or("");
    let (title, _) = label.split_once('\t').unwrap_or((label, ""));
    if title == "Zoom" {
        let row_top = rect.y + dip(8.0) + row_offset(&state.menu_labels, index);
        let row = Rect { x: rect.x + dip(4.0), y: row_top, w: rect.w - dip(8.0), h: dip(32.0) };
        let mut cursor = row.right() - dip(156.0);
        let widths = [dip(28.0), dip(52.0), dip(28.0), dip(28.0)];
        let kinds = [-1, 0, 1, 2];
        for (width, kind) in widths.into_iter().zip(kinds) {
            if x >= cursor && x < cursor + width {
                return Some(Hit::Zoom(kind));
            }
            cursor += width;
        }
        return Some(Hit::Panel);
    }
    Some(Hit::MenuItem(index))
}

fn row_offset(labels: &[String], index: usize) -> f32 {
    let mut y = 0.0;
    for label in labels.iter().take(index) {
        y += if label.is_empty() { dip(9.0) } else { dip(32.0) };
    }
    y
}

fn menu_index_at(labels: &[String], dy: f32) -> Option<usize> {
    let mut y = dip(8.0);
    for (index, label) in labels.iter().enumerate() {
        let row_h = if label.is_empty() { dip(9.0) } else { dip(32.0) };
        if dy >= y && dy < y + row_h && !label.is_empty() {
            return Some(index);
        }
        y += row_h;
    }
    None
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
    let w = dip(336.0);
    let h = menu_block_height(&state.menu_labels);
    match popup {
        Popup::App => {
            let tools = toolbar(state.width, state.scale, state.show_home);
            Rect {
                x: (tools.menu.right() - w).max(dip(8.0)),
                y: tools.menu.bottom() + dip(4.0),
                w,
                h,
            }
        }
        Popup::Context => Rect {
            x: state.pointer.0.min(state.width - w - dip(8.0)).max(dip(8.0)),
            y: state.pointer.1.min(state.height - h - dip(8.0)).max(snap(TAB_H, paint_scale())),
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
    let bubble = Rect { x: dip(8.0), y: state.height - dip(32.0), w: text_w + dip(20.0), h: dip(24.0) };
    fill_round(pixels, width, height, bubble.x, bubble.y, bubble.w, bubble.h, dip(4.0), BUBBLE);
    stroke_round(pixels, width, height, bubble.x, bubble.y, bubble.w, bubble.h, dip(4.0), LINE);
    draw_text_fade(
        pixels,
        width,
        height,
        state.fonts,
        bubble.x + dip(10.0),
        bubble.y + dip(16.0),
        12.0,
        MUTED,
        text,
        bubble.x + dip(10.0),
        bubble.right() - dip(8.0),
        dip(14.0),
    );
}

fn tip(pixels: &mut [u32], width: u32, height: u32, state: &State<'_>, anchor: Rect, label: &str) {
    if state.pressed || !anchor.contains(state.pointer.0, state.pointer.1) {
        return;
    }
    let text_w = text_width(state.fonts, label, 12.0);
    let bubble_w = text_w + dip(16.0);
    let mut x = anchor.x + anchor.w / 2.0 - bubble_w / 2.0;
    let y = anchor.bottom() + dip(6.0);
    x = x.clamp(dip(4.0), (state.width - bubble_w - dip(4.0)).max(dip(4.0)));
    fill_round(pixels, width, height, x, y, bubble_w, dip(22.0), dip(4.0), rgb(60, 64, 67));
    draw_text(pixels, width, height, state.fonts, x + dip(8.0), y + dip(15.0), 12.0, TOOLBAR, label);
}

fn omnibox_text(url: &str) -> &str {
    match url {
        "" | "about:blank" | "about:newtab" | "about:mithu" | "about:home" => "",
        other => other,
    }
}

fn shadow(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32) {
    fill_round(pixels, width, height, x + dip(1.0), y + dip(2.0), w, h, dip(12.0), rgb(230, 230, 230));
}

fn blit_favicon(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, dest: f32, fw: u32, fh: u32, bytes: &[u8]) {
    if fw == 0 || fh == 0 {
        return;
    }
    let dest_px = dest.round().max(1.0) as u32;
    for row in 0..dest_px {
        for col in 0..dest_px {
            let sx = (col * fw / dest_px).min(fw - 1);
            let sy = (row * fh / dest_px).min(fh - 1);
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
    let size = snap(size, paint_scale()).max(1.0);
    draw_text_fade(pixels, width, height, fonts, x.round(), baseline.round(), size, color, text, f32::MIN, f32::MAX, 0.0);
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
    let size = snap(size, paint_scale()).max(1.0);
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
    let size = snap(size, paint_scale()).max(1.0);
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
            let sd = sd_round(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, radius);
            let cover = cover_from_sd(sd);
            if cover == 255 {
                put(pixels, width, height, px, py, color);
            } else if cover > 0 {
                blend(pixels, width, height, px, py, color, cover);
            }
        }
    }
}

fn stroke_round(pixels: &mut [u32], width: u32, height: u32, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32) {
    let radius = radius.max(0.0).min(w / 2.0).min(h / 2.0);
    let x0 = x.floor().max(0.0) as i32;
    let y0 = y.floor().max(0.0) as i32;
    let x1 = (x + w).ceil().min(width as f32) as i32;
    let y1 = (y + h).ceil().min(height as f32) as i32;
    for py in y0..y1 {
        for px in x0..x1 {
            let sx = px as f32 + 0.5;
            let sy = py as f32 + 0.5;
            let outer = sd_round(sx, sy, x, y, w, h, radius);
            let inner = sd_round(sx, sy, x + 1.0, y + 1.0, (w - 2.0).max(0.0), (h - 2.0).max(0.0), (radius - 1.0).max(0.0));
            let cover = cover_from_sd(outer).saturating_sub(cover_from_sd(inner));
            if cover > 0 {
                blend(pixels, width, height, px, py, color, cover);
            }
        }
    }
}

fn sd_round(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, radius: f32) -> f32 {
    let radius = radius.max(0.0).min(w * 0.5).min(h * 0.5);
    let dx = (px - (x + w * 0.5)).abs() - (w * 0.5 - radius);
    let dy = (py - (y + h * 0.5)).abs() - (h * 0.5 - radius);
    dx.max(dy).min(0.0) + dx.max(0.0).hypot(dy.max(0.0)) - radius
}

fn cover_from_sd(sd: f32) -> u8 {
    ((0.5 - sd).clamp(0.0, 1.0) * 255.0).round() as u8
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
            let sd = dx.hypot(dy) - radius;
            let cover = ((cover_from_sd(sd) as u32) * alpha as u32 / 255) as u8;
            if cover == 255 {
                put(pixels, width, height, px, py, color);
            } else if cover > 0 {
                blend(pixels, width, height, px, py, color, cover);
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
                let mut hits = 0u8;
                for sy in 0..2 {
                    for sx in 0..2 {
                        let sample_x = x as f32 + (sx as f32 + 0.5) * 0.5;
                        let sample_y = y as f32 + (sy as f32 + 0.5) * 0.5;
                        if point_in_polygon(sample_x, sample_y, points) {
                            hits += 1;
                        }
                    }
                }
                if hits == 4 {
                    put(pixels, width, height, x, y, color);
                } else if hits > 0 {
                    blend(pixels, width, height, x, y, color, (hits as u32 * 255 / 4) as u8);
                }
            }
            index += 2;
        }
    }
}

fn point_in_polygon(x: f32, y: f32, points: &[(f32, f32)]) -> bool {
    let mut inside = false;
    for index in 0..points.len() {
        let (x1, y1) = points[index];
        let (x2, y2) = points[(index + 1) % points.len()];
        if (y1 > y) != (y2 > y) {
            let x_at = (x2 - x1) * (y - y1) / (y2 - y1) + x1;
            if x < x_at {
                inside = !inside;
            }
        }
    }
    inside
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
        let tools = toolbar(1280.0, 1.0, true);
        assert!(tools.home.x > tools.reload.x);
        assert!(tools.address.x > tools.home.x);
        assert!(tools.star.x > tools.address.x);
        assert!(tools.star.right() <= tools.address.right() + 1.0);
        assert!(tools.extensions.x >= tools.address.right() - 1.0);
        assert!(tools.menu.right() <= 1280.0);
    }

    #[test]
    fn tab_title_is_clipped_inside_the_tab_and_captions_stay_clear() {
        let (tabs, _plus, search) = tab_layout(1280.0, 1, 1.0);
        let tab = &tabs[0];
        assert!(tab.title_right < tab.close.x);
        assert!(tab.close.right() <= tab.rect.right());
        assert!(tab.rect.w <= 240.0);
        assert!(search.right() < 1280.0 - CAPTION_W * 3.0);
        let (many, plus, _) = tab_layout(1280.0, 30, 1.0);
        assert!(many.last().unwrap().rect.right() <= plus.x + 1.0);
        assert!(plus.right() < 1280.0 - CAPTION_W * 3.0);
    }

    #[test]
    fn fractional_scales_land_on_whole_pixels() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let height = chrome_height(true, scale);
            assert_eq!(height, height.round());
            let (tabs, plus, search) = tab_layout(1100.0 * scale, 3, scale);
            for tab in &tabs {
                assert_eq!(tab.rect.x, tab.rect.x.round());
                assert_eq!(tab.rect.y, tab.rect.y.round());
                assert_eq!(tab.rect.w, tab.rect.w.round());
                assert_eq!(tab.close.x, tab.close.x.round());
            }
            assert_eq!(plus.x, plus.x.round());
            assert_eq!(search.y, search.y.round());
            let tools = toolbar(1100.0 * scale, scale, true);
            assert_eq!(tools.address.x, tools.address.x.round());
            assert_eq!(tools.address.h, tools.address.h.round());
            assert!(tools.home.x > tools.reload.x);
            assert!(tools.address.x >= tools.home.right() - 1.0);
            let hair = height - 1.0;
            assert_eq!(hair, hair.round());
            PAINT_SCALE.set(scale);
            let fonts = mithu_text::FontBook::bundled();
            let state = State {
                width: 1100.0 * scale,
                height: 720.0 * scale,
                tabs: Vec::new(),
                active: 0,
                omnibox: String::new(),
                omnibox_focused: false,
                cursor: 0,
                secure: Secure::Https,
                bookmarked: false,
                bookmarks: Vec::new(),
                bookmarks_open: true,
                can_back: false,
                can_forward: false,
                loading: false,
                progress: 0.0,
                link_status: String::new(),
                pointer: (0.0, 0.0),
                pressed: false,
                ntp: false,
                menu: Some(Popup::App),
                menu_labels: vec!["New tab".into(), "History".into(), "Downloads".into(), "Settings".into()],
                submenu_at: None,
                submenu_labels: Vec::new(),
                tab_search: false,
                extensions: false,
                profile: false,
                suggestions: Vec::new(),
                suggestion: None,
                find_open: false,
                find_query: String::new(),
                fonts: &fonts,
                scale,
                dark: false,
                show_home: true,
            };
            let menu = menu_rect(&state, Popup::App);
            assert_eq!(menu.w, (336.0 * scale).round());
            assert!(menu.right() <= state.width + 0.5);
            assert!(menu.x >= 0.0);
            let label = text_width(&fonts, "Downloads", 13.0);
            assert!(dip(32.0) + label < menu.w, "menu text fits at {scale}");
        }
    }
}
