//! Browser shell: navigation, history, chrome, and a headless frame renderer.
//!
//! The window crate only forwards input. Loading, layout, and painting live here
//! so the engine can be tested without a display server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mithu_base::{Color, Rect};
use mithu_dom::{Document, NodeId};
use mithu_html::parse_html;
use mithu_image::Bitmap;
use mithu_js::{Session, Submission};
use mithu_layout::{LayoutBox, LayoutTree, hit_test, layout_tree};
use mithu_net::{get, post};
use mithu_paint::{Canvas, paint_layout};
use mithu_style::{ComputedStyle, StyleState, font_faces, style_with};
use mithu_text::{FontBook, MeasureText};
use mithu_url::{Url, UrlError};

mod icons;
mod process;

/// `kTabStripHeight` in `chrome/browser/ui/layout_constants.cc`: tab height 35
/// plus the 6px strip padding. The toolbar overlap is the extra pixel the
/// active tab draws into the toolbar.
pub const TAB_H: f32 = 41.0;
pub const TOOLBAR_H: f32 = 48.0;
/// Bookmark button height 28 plus the 6px bar margin (`kBookmarkBarHeight`).
pub const BOOKMARK_BAR_H: f32 = 34.0;
pub const CONTENT_TOP: f32 = TAB_H + TOOLBAR_H + BOOKMARK_BAR_H;
pub const FIND_H: f32 = 36.0;
pub const STATUS_H: f32 = 26.0;
/// Gap between the page and the status line so the last text line is not covered.
const PAGE_GAP: f32 = 52.0;
/// `kToolbarButtonHeight`.
const BTN: f32 = 34.0;
const CAPTION_W: f32 = 46.0;

/// Default light frame from `ui/color/sys_color_mixer.cc`: `kColorSysHeader`
/// is `kColorRefPrimary90` unless the user picks the grayscale scheme.
/// `kColorSysBase` (the active tab) is `kColorRefNeutral100`.
const TAB_STRIP: Color = Color::rgb(211, 227, 253);
const TOOLBAR: Color = Color::rgb(255, 255, 255);
const ICON: Color = Color::rgb(31, 31, 31);
const ICON_DISABLED: Color = Color::rgb(143, 143, 143);
const OMNIBOX: Color = Color::rgb(239, 237, 237);
const ADDRESS: Color = Color::rgb(255, 255, 255);
const ADDRESS_BORDER: Color = Color::rgb(199, 199, 199);
const ADDRESS_FOCUS: Color = Color::rgb(11, 87, 208);
const STATUS_BG: Color = Color::rgb(242, 242, 242);
const STATUS_TEXT: Color = Color::rgb(94, 94, 94);
const SELECTION: Color = Color::rgb(211, 227, 253);
const FIND_HIT: Color = Color::rgb(255, 214, 64);
const FIND_ACTIVE: Color = Color::rgb(255, 140, 40);
const FIND_BAR: Color = Color::rgb(255, 255, 255);
const STAR: Color = Color::rgb(242, 180, 0);
const BOOKMARKED: Color = Color::rgb(254, 239, 195);
const SEPARATOR: Color = Color::rgb(199, 199, 199);
const CHIP: Color = Color::rgb(239, 237, 237);
const INK: Color = Color::rgb(31, 31, 31);
/// `kColorSysStateHeaderHover` = `kColorRefPrimary80`.
const HOVER: Color = Color::rgb(168, 199, 250);
/// `kColorSysHeaderContainer` = `kColorRefPrimary95`. New-tab button fill.
const PLUS: Color = Color::rgb(236, 243, 254);
const NOT_SECURE: Color = Color::rgb(95, 99, 104);
const ZOOMS: [f32; 17] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0,
];

struct Page {
    history: Vec<Url>,
    index: usize,
    current: Url,
    base: Url,
    omnibox: String,
    cursor: usize,
    select_all: bool,
    scroll: f32,
    title: String,
    status: String,
    document: Document,
    styles: Vec<ComputedStyle>,
    layout: LayoutTree,
    images: HashMap<NodeId, Arc<Bitmap>>,
    session: Option<Session>,
    focused: Option<NodeId>,
    find_query: String,
    find_open: bool,
    find_at: usize,
    zoom: f32,
    author_css: String,
    hover: Option<NodeId>,
    /// Last document generation that was styled. `None` until the first paint.
    painted_revision: Option<u64>,
    pinned: bool,
    group: Option<u8>,
}

struct Tabs {
    pages: Vec<Page>,
    active: usize,
}

impl Tabs {
    fn page(&self) -> &Page {
        &self.pages[self.active]
    }

    fn page_mut(&mut self) -> &mut Page {
        &mut self.pages[self.active]
    }
}

impl Page {
    fn blank(width: f32) -> Self {
        let blank = Url::parse("about:blank").expect("about:blank");
        Self {
            history: Vec::new(),
            index: 0,
            current: blank.clone(),
            base: blank,
            omnibox: String::new(),
            cursor: 0,
            select_all: false,
            scroll: 0.0,
            title: String::new(),
            status: String::new(),
            document: Document::new(),
            styles: Vec::new(),
            layout: layout_tree(
                &Document::new(),
                &[],
                width,
                600.0,
                &mithu_text::NaiveMetrics,
                &HashMap::new(),
            ),
            images: HashMap::new(),
            session: None,
            focused: None,
            find_query: String::new(),
            find_open: false,
            find_at: 0,
            zoom: 1.0,
            author_css: String::new(),
            hover: None,
            painted_revision: None,
            pinned: false,
            group: None,
        }
    }
}

struct ChromeColors {
    strip: Color,
    toolbar: Color,
    active: Color,
    icon: Color,
    icon_disabled: Color,
    omnibox: Color,
    address: Color,
    focus: Color,
    ink: Color,
    muted: Color,
    status_bg: Color,
    status_text: Color,
    chip: Color,
    plus: Color,
    hover: Color,
    menu: Color,
    tooltip_bg: Color,
    tooltip_ink: Color,
}

fn os_prefers_dark() -> bool {
    if let Ok(value) = std::env::var("MITHU_THEME") {
        return value.eq_ignore_ascii_case("dark");
    }
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output();
    if let Ok(output) = output {
        let text = String::from_utf8_lossy(&output.stdout);
        if text.contains("prefer-dark") {
            return true;
        }
        if text.contains("prefer-light") || text.contains("default") {
            return false;
        }
    }
    std::fs::read_to_string("/home/ubuntu/.config/gtk-3.0/settings.ini")
        .or_else(|_| std::fs::read_to_string("/home/ubuntu/.config/gtk-4.0/settings.ini"))
        .is_ok_and(|text| text.contains("gtk-application-prefer-dark=1"))
}

/// Active-tab outline from `HorizontalTabStyleViews::GetPath`.
///
/// The bottom corners are concave so the tab joins the toolbar. Points are
/// sampled from the same quarter-circles the Skia path builds with `arcTo`.
fn active_tab_contour(rect: Rect) -> Vec<(f32, f32)> {
    let top_radius = 8.0_f32;
    let scoop = 10.0_f32;
    let left = rect.x;
    let right = rect.right();
    let top = rect.y;
    let bottom = rect.bottom();
    let tab_left = left + scoop;
    let tab_right = right - scoop;
    let mut points = Vec::new();
    points.push((left, bottom));
    arc_points(
        &mut points,
        tab_left,
        bottom - scoop,
        scoop,
        std::f32::consts::FRAC_PI_2,
        std::f32::consts::PI,
    );
    points.push((tab_left, top + top_radius));
    arc_points(
        &mut points,
        tab_left + top_radius,
        top + top_radius,
        top_radius,
        std::f32::consts::PI,
        std::f32::consts::PI * 1.5,
    );
    points.push((tab_right - top_radius, top));
    arc_points(
        &mut points,
        tab_right - top_radius,
        top + top_radius,
        top_radius,
        std::f32::consts::PI * 1.5,
        std::f32::consts::TAU,
    );
    points.push((tab_right, bottom - scoop));
    arc_points(
        &mut points,
        tab_right,
        bottom - scoop,
        scoop,
        std::f32::consts::PI,
        std::f32::consts::FRAC_PI_2,
    );
    points.push((right, bottom));
    points
}

fn arc_points(points: &mut Vec<(f32, f32)>, cx: f32, cy: f32, radius: f32, start: f32, end: f32) {
    let steps = 6;
    for step in 0..=steps {
        let t = start + (end - start) * (step as f32 / steps as f32);
        points.push((cx + t.cos() * radius, cy + t.sin() * radius));
    }
}

fn chrome_colors(dark: bool) -> ChromeColors {
    if dark {
        return ChromeColors {
            strip: Color::rgb(31, 32, 32),
            toolbar: Color::rgb(31, 31, 31),
            active: Color::rgb(42, 42, 42),
            icon: Color::rgb(227, 227, 227),
            icon_disabled: Color::rgb(143, 143, 143),
            omnibox: Color::rgb(42, 42, 42),
            address: Color::rgb(31, 31, 31),
            focus: ADDRESS_FOCUS,
            ink: Color::rgb(227, 227, 227),
            muted: Color::rgb(196, 199, 197),
            status_bg: Color::rgb(31, 32, 32),
            status_text: Color::rgb(196, 199, 197),
            chip: Color::rgb(60, 60, 60),
            plus: Color::rgb(60, 60, 60),
            hover: Color::rgb(60, 60, 60),
            menu: Color::rgb(42, 42, 42),
            tooltip_bg: Color::rgb(241, 243, 244),
            tooltip_ink: Color::rgb(31, 31, 31),
        };
    }
    ChromeColors {
        strip: TAB_STRIP,
        toolbar: TOOLBAR,
        active: Color::WHITE,
        icon: ICON,
        icon_disabled: ICON_DISABLED,
        omnibox: OMNIBOX,
        address: ADDRESS,
        focus: ADDRESS_FOCUS,
        ink: INK,
        muted: NOT_SECURE,
        status_bg: STATUS_BG,
        status_text: STATUS_TEXT,
        chip: CHIP,
        plus: PLUS,
        hover: HOVER,
        menu: ADDRESS,
        tooltip_bg: Color::rgb(32, 33, 36),
        tooltip_ink: Color::WHITE,
    }
}

const GROUP_COLORS: [Color; 8] = [
    Color::rgb(11, 87, 208),
    Color::rgb(179, 38, 30),
    Color::rgb(196, 132, 0),
    Color::rgb(24, 128, 56),
    Color::rgb(208, 24, 132),
    Color::rgb(147, 52, 230),
    Color::rgb(0, 123, 131),
    Color::rgb(232, 113, 10),
];

pub enum FrameHit {
    Close,
    Minimize,
    Maximize,
    Drag,
}

struct Bookmark {
    url: Url,
    title: String,
}

pub struct Browser {
    fonts: FontBook,
    viewport_w: f32,
    viewport_h: f32,
    pub omnibox_focused: bool,
    pub find_focused: bool,
    bookmarks: Vec<Bookmark>,
    bookmarks_open: bool,
    suggestion: Option<usize>,
    menu: Option<Menu>,
    tabs: Tabs,
    visits: Vec<(String, String)>,
    downloads: Vec<(String, String)>,
    drag: Option<usize>,
    dark: bool,
    cursor: (f32, f32),
    hovering: bool,
    ripple: Option<(f32, f32, Instant)>,
    cooperative: bool,
    script_refresh: Instant,
    opened: Instant,
    closed: Vec<Url>,
    local_data: HashMap<String, HashMap<String, String>>,
    session_data: HashMap<String, HashMap<String, String>>,
    js_hops: u8,
}

struct Fetched {
    url: Url,
    html: String,
    status: String,
    download_name: Option<String>,
}

impl Browser {
    fn page(&self) -> &Page {
        self.tabs.page()
    }

    fn page_mut(&mut self) -> &mut Page {
        self.tabs.page_mut()
    }

    pub fn status_line(&self) -> &str {
        &self.page().status
    }

    pub fn script_logs(&self) -> Vec<String> {
        self.page()
            .session
            .as_ref()
            .map(|session| session.logs())
            .unwrap_or_default()
    }

    pub fn new() -> Self {
        let mut browser = Self {
            fonts: FontBook::load(),
            viewport_w: 1200.0,
            viewport_h: 800.0,
            omnibox_focused: false,
            find_focused: false,
            bookmarks: Vec::new(),
            bookmarks_open: false,
            suggestion: None,
            menu: None,
            tabs: Tabs {
                pages: vec![Page::blank(1200.0)],
                active: 0,
            },
            visits: Vec::new(),
            downloads: Vec::new(),
            drag: None,
            dark: false,
            cursor: (0.0, 0.0),
            hovering: false,
            ripple: None,
            cooperative: false,
            script_refresh: Instant::now(),
            opened: Instant::now(),
            closed: Vec::new(),
            local_data: HashMap::new(),
            session_data: HashMap::new(),
            js_hops: 0,
        };
        browser.navigate("about:mithu");
        browser
    }

    /// Script tasks yield back to the window instead of running to completion
    /// inside `navigate`. Screenshots leave this off so the picture includes
    /// the finished page.
    pub fn chrome_animating(&self) -> bool {
        self.page().status.starts_with("Loading")
            || self
                .ripple
                .is_some_and(|(_, _, started)| started.elapsed().as_secs_f32() < 0.35)
    }

    pub fn set_cooperative_scripts(&mut self, enabled: bool) {
        self.cooperative = enabled;
    }

    /// Match the frame to the desktop color scheme. The menu toggle still wins
    /// after the user picks a theme.
    pub fn follow_os_theme(&mut self) {
        self.dark = os_prefers_dark();
    }

    /// Run pending script tasks for `budget`, then refresh the page.
    /// Returns true while scripts remain, so the window keeps painting.
    pub fn content_revision(&self) -> Option<u64> {
        self.page().session.as_ref().map(Session::revision)
    }

    pub fn pump_scripts(&mut self, budget: Duration) -> bool {
        let pending = {
            let Some(session) = self.page_mut().session.as_mut() else {
                return false;
            };
            if !session.scripts_pending() {
                return false;
            }
            session.pump(budget)
        };
        if pending {
            self.page_mut().status = "Loading".to_string();
            if self.script_refresh.elapsed() >= Duration::from_millis(200) {
                self.script_refresh = Instant::now();
                self.pull_page();
                self.page_mut().status = "Loading".to_string();
            }
            return true;
        }
        let next = self
            .page_mut()
            .session
            .as_mut()
            .and_then(|session| session.take_navigation());
        self.pull_page();
        if let Some(next) = next
            && self.js_hops < 8
        {
            self.js_hops += 1;
            self.navigate(&next);
        }
        false
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.pages.len()
    }

    pub fn active_tab(&self) -> usize {
        self.tabs.active
    }

    pub fn new_tab(&mut self) {
        self.omnibox_focused = false;
        self.find_focused = false;
        self.tabs.pages.push(Page::blank(self.viewport_w));
        self.tabs.active = self.tabs.pages.len() - 1;
        self.navigate("about:mithu");
    }

    pub fn select_tab(&mut self, index: usize) {
        if index >= self.tabs.pages.len() {
            return;
        }
        self.omnibox_focused = false;
        self.find_focused = false;
        self.tabs.active = index;
    }

    pub fn next_tab(&mut self) {
        if self.tabs.pages.is_empty() {
            return;
        }
        let next = (self.tabs.active + 1) % self.tabs.pages.len();
        self.select_tab(next);
    }

    pub fn previous_tab(&mut self) {
        let count = self.tabs.pages.len();
        if count == 0 {
            return;
        }
        let next = if self.tabs.active == 0 {
            count - 1
        } else {
            self.tabs.active - 1
        };
        self.select_tab(next);
    }

    pub fn toggle_dark(&mut self) {
        self.dark = !self.dark;
    }

    pub fn is_dark(&self) -> bool {
        self.dark
    }

    pub fn reopen_closed_tab(&mut self) {
        let Some(url) = self.closed.pop() else {
            return;
        };
        self.new_tab();
        self.navigate_url(url);
    }

    pub fn frame_hit(&self, x: f32, y: f32) -> Option<FrameHit> {
        if y < 0.0 || y >= TAB_H || x < 0.0 || x >= self.viewport_w {
            return None;
        }
        let [minimize, maximize, close] = caption_rects(self.viewport_w);
        if close.contains(x, y) {
            return Some(FrameHit::Close);
        }
        if maximize.contains(x, y) {
            return Some(FrameHit::Maximize);
        }
        if minimize.contains(x, y) {
            return Some(FrameHit::Minimize);
        }
        let strip = self.tab_strip();
        if strip.plus.contains(x, y)
            || strip.search.contains(x, y)
            || strip.tabs.iter().any(|rect| rect.contains(x, y))
        {
            return None;
        }
        Some(FrameHit::Drag)
    }

    pub fn toggle_bookmark(&mut self) {
        let url = self.page().current.clone();
        let key = url.to_display();
        if let Some(index) = self
            .bookmarks
            .iter()
            .position(|item| item.url.to_display() == key)
        {
            self.bookmarks.remove(index);
            return;
        }
        let title = self.page().title.clone();
        let title = if title.is_empty() { key } else { title };
        self.bookmarks.push(Bookmark { url, title });
    }

    pub fn toggle_bookmark_list(&mut self) {
        self.bookmarks_open = !self.bookmarks_open;
        if self.bookmarks_open {
            self.omnibox_focused = false;
            self.suggestion = None;
        }
    }

    pub fn close_bookmarks(&mut self) {
        self.bookmarks_open = false;
    }

    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    pub fn close_menu(&mut self) {
        self.menu = None;
    }

    pub fn bookmarks_open(&self) -> bool {
        self.bookmarks_open
    }

    pub fn bookmark_count(&self) -> usize {
        self.bookmarks.len()
    }

    pub fn is_bookmarked(&self) -> bool {
        let key = self.page().current.to_display();
        self.bookmarks
            .iter()
            .any(|item| item.url.to_display() == key)
    }

    pub fn zoom(&self) -> f32 {
        self.page().zoom
    }

    pub fn zoom_in(&mut self) {
        self.step_zoom(1);
    }

    pub fn zoom_out(&mut self) {
        self.step_zoom(-1);
    }

    pub fn zoom_reset(&mut self) {
        self.set_zoom(1.0);
    }

    fn step_zoom(&mut self, direction: i32) {
        let current = self.page().zoom;
        let next = if direction > 0 {
            ZOOMS
                .into_iter()
                .find(|level| *level > current + 0.001)
                .unwrap_or(ZOOMS[ZOOMS.len() - 1])
        } else {
            ZOOMS
                .into_iter()
                .rev()
                .find(|level| *level < current - 0.001)
                .unwrap_or(ZOOMS[0])
        };
        self.set_zoom(next);
    }

    fn set_zoom(&mut self, zoom: f32) {
        self.page_mut().zoom = zoom;
        self.relayout();
        let max = self.max_scroll();
        let page = self.page_mut();
        page.scroll = page.scroll.clamp(0.0, max);
    }

    pub fn close_tab(&mut self) {
        self.close_tab_at(self.tabs.active);
    }

    fn close_tab_at(&mut self, index: usize) {
        if index >= self.tabs.pages.len() {
            return;
        }
        self.omnibox_focused = false;
        self.find_focused = false;
        let url = self.tabs.pages[index].current.clone();
        if self.closed.len() == 16 {
            self.closed.remove(0);
        }
        self.closed.push(url);
        if self.tabs.pages.len() == 1 {
            self.navigate("about:mithu");
            return;
        }
        self.tabs.pages.remove(index);
        if self.tabs.active > index || self.tabs.active >= self.tabs.pages.len() {
            self.tabs.active = self.tabs.active.saturating_sub(1);
        }
    }

    pub fn navigate(&mut self, input: &str) {
        self.navigate_url(interpret(input));
    }

    pub fn back(&mut self) {
        let url = {
            let page = self.page_mut();
            if page.index == 0 || page.history.is_empty() {
                return;
            }
            page.index -= 1;
            page.history[page.index].clone()
        };
        self.load_url(url, false);
    }

    pub fn forward(&mut self) {
        let url = {
            let page = self.page_mut();
            if page.index + 1 >= page.history.len() {
                return;
            }
            page.index += 1;
            page.history[page.index].clone()
        };
        self.load_url(url, false);
    }

    pub fn reload(&mut self) {
        let url = self.page().current.clone();
        self.load_url(url, true);
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.viewport_w = width.max(1.0);
        self.viewport_h = height.max(1.0);
        self.relayout();
        let max = self.max_scroll();
        let page = self.page_mut();
        page.scroll = page.scroll.clamp(0.0, max);
    }

    pub fn scroll_by(&mut self, delta: f32) {
        let zoom = self.page().zoom.max(0.01);
        let max = self.max_scroll();
        let page = self.page_mut();
        page.scroll = (page.scroll + delta / zoom).clamp(0.0, max);
    }

    pub fn scroll_page(&mut self, pages: f32) {
        self.scroll_by(pages * self.page_height() * 0.9);
    }

    pub fn context_menu(&mut self, x: f32, y: f32) {
        self.bookmarks_open = false;
        self.omnibox_focused = false;
        self.suggestion = None;
        if y < TAB_H {
            let strip = self.tab_strip();
            if let Some(index) = strip.tabs.iter().position(|rect| rect.contains(x, y)) {
                let pinned = self.tabs.pages[index].pinned;
                let mut items = vec![("Close tab".to_string(), MenuAction::CloseTab(index))];
                items.push((
                    if pinned {
                        "Unpin tab".to_string()
                    } else {
                        "Pin tab".to_string()
                    },
                    MenuAction::Pin(index),
                ));
                items.push(("Add to new group".to_string(), MenuAction::Group(index)));
                if !self.closed.is_empty() {
                    items.push(("Reopen closed tab".to_string(), MenuAction::Reopen));
                }
                self.menu = Some(Menu { x, y, items });
                return;
            }
        }
        let mut items = Vec::new();
        if y >= CONTENT_TOP && y < self.viewport_h - STATUS_H {
            let zoom = self.page().zoom.max(0.01);
            let doc_x = x / zoom;
            let doc_y = (y - CONTENT_TOP) / zoom + self.page().scroll;
            if let Some(href) = self.hit_link(doc_x, doc_y) {
                items.push(("Open link".to_string(), MenuAction::Open(href.clone())));
                items.push(("Open in new tab".to_string(), MenuAction::NewTab(href)));
            }
        }
        if self.can_back() {
            items.push(("Back".to_string(), MenuAction::Back));
        }
        if self.can_forward() {
            items.push(("Forward".to_string(), MenuAction::Forward));
        }
        items.push(("Reload".to_string(), MenuAction::Reload));
        items.push(("Bookmark this page".to_string(), MenuAction::Bookmark));
        self.menu = Some(Menu { x, y, items });
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) {
        if let Some(menu) = self.menu.take() {
            let bounds = menu_bounds(&menu, self.viewport_w, self.viewport_h);
            if bounds.contains(x, y) {
                let index = ((y - bounds.y) / 32.0).floor() as usize;
                if let Some((_, action)) = menu.items.get(index).cloned() {
                    self.run_menu(action);
                }
            }
            return;
        }
        if y >= self.viewport_h - STATUS_H {
            return;
        }
        if self.page().find_open {
            let bar = find_bar_rect(self.viewport_w, self.viewport_h);
            if bar.contains(x, y) {
                self.find_focused = true;
                self.omnibox_focused = false;
                return;
            }
        }
        self.ripple = Some((x, y, Instant::now()));
        if y < TAB_H {
            let strip = self.tab_strip();
            if strip.search.contains(x, y) {
                self.open_tab_search();
            } else if strip.plus.contains(x, y) {
                self.new_tab();
            } else if let Some(index) = strip
                .closes
                .iter()
                .position(|rect| rect.width > 0.0 && rect.contains(x, y))
            {
                let visible = index == self.tabs.active
                    || strip
                        .tabs
                        .get(index)
                        .is_some_and(|rect| rect.contains(self.cursor.0, self.cursor.1));
                if visible && !self.tabs.pages[index].pinned {
                    self.close_tab_at(index);
                } else if let Some(rect) = strip.tabs.get(index)
                    && rect.contains(x, y)
                {
                    self.select_tab(index);
                    self.drag = Some(index);
                }
            } else if let Some(index) = strip.tabs.iter().position(|rect| rect.contains(x, y)) {
                self.select_tab(index);
                self.drag = Some(index);
            }
            return;
        }
        if y < TAB_H + TOOLBAR_H {
            let bar = toolbar(self.viewport_w);
            let chip = security_chip(&self.page().current, self.omnibox_focused);
            let chip_rect = chip_rect(bar.address, &chip, &self.fonts);
            if bar.back.contains(x, y) && self.can_back() {
                self.back();
            } else if bar.forward.contains(x, y) && self.can_forward() {
                self.forward();
            } else if bar.reload.contains(x, y) {
                self.reload();
            } else if bar.home.contains(x, y) {
                self.navigate("about:mithu");
            } else if bar.star.contains(x, y) {
                self.toggle_bookmark();
            } else if bar.extensions.contains(x, y) {
                self.open_extensions_menu();
            } else if bar.profile.contains(x, y) {
                self.open_profile_menu();
            } else if bar.menu.contains(x, y) {
                self.open_app_menu();
            } else if chip_rect.contains(x, y) {
                self.open_site_info();
            } else if bar.address.contains(x, y) {
                self.focus_omnibox();
            }
            return;
        }
        if y < CONTENT_TOP {
            let slots = bookmark_slots(self.viewport_w, self.bookmarks.len());
            if let Some(index) = slots.iter().position(|rect| rect.contains(x, y))
                && let Some(bookmark) = self.bookmarks.get(index)
            {
                let url = bookmark.url.clone();
                self.navigate_url(url);
            }
            return;
        }
        if let Some(shelf) = self.shelf_rect()
            && shelf.contains(x, y)
        {
            self.navigate("about:downloads");
            return;
        }
        if self.omnibox_focused {
            let items = self.suggestions();
            let panel = suggestion_panel(self.viewport_w, items.len());
            if !items.is_empty() && panel.contains(x, y) {
                let index = ((y - panel.y) / 32.0).floor() as usize;
                if let Some(item) = items.get(index) {
                    let url = item.url.clone();
                    self.omnibox_focused = false;
                    self.suggestion = None;
                    self.navigate_url(url);
                }
                return;
            }
        }
        if self.bookmarks_open {
            let panel = bookmark_panel(self.viewport_w, self.bookmarks.len());
            if panel.contains(x, y) {
                let index = ((y - panel.y) / 32.0).floor() as usize;
                if let Some(bookmark) = self.bookmarks.get(index) {
                    let url = bookmark.url.clone();
                    self.bookmarks_open = false;
                    self.navigate_url(url);
                }
                return;
            }
            self.bookmarks_open = false;
        }
        self.omnibox_focused = false;
        self.find_focused = false;
        self.suggestion = None;
        let scroll = self.page().scroll;
        let zoom = self.page().zoom.max(0.01);
        self.page_mut().select_all = false;
        let doc_x = x / zoom;
        let doc_y = (y - CONTENT_TOP) / zoom + scroll;
        self.activate(doc_x, doc_y);
    }

    fn run_menu(&mut self, action: MenuAction) {
        match action {
            MenuAction::Open(href) => self.follow(href),
            MenuAction::NewTab(href) => self.open_in_new_tab(href),
            MenuAction::Back => self.back(),
            MenuAction::Forward => self.forward(),
            MenuAction::Reload => self.reload(),
            MenuAction::Bookmark => self.toggle_bookmark(),
            MenuAction::NewTabPage => self.new_tab(),
            MenuAction::Bookmarks => self.toggle_bookmark_list(),
            MenuAction::History => self.navigate("about:history"),
            MenuAction::Downloads => self.navigate("about:downloads"),
            MenuAction::NewWindow => self.new_tab(),
            MenuAction::Find => self.open_find(),
            MenuAction::ZoomIn => self.zoom_in(),
            MenuAction::ZoomOut => self.zoom_out(),
            MenuAction::ZoomReset => self.zoom_reset(),
            MenuAction::Settings => self.navigate("about:settings"),
            MenuAction::Dark => self.toggle_dark(),
            MenuAction::CloseTab(index) => self.close_tab_at(index),
            MenuAction::Pin(index) => {
                if let Some(page) = self.tabs.pages.get_mut(index) {
                    page.pinned = !page.pinned;
                }
            }
            MenuAction::Group(index) => {
                if let Some(page) = self.tabs.pages.get_mut(index) {
                    let color = page.group.map(|value| (value + 1) % 8).unwrap_or(0);
                    page.group = Some(color);
                }
            }
            MenuAction::Select(index) => self.select_tab(index),
            MenuAction::Reopen => self.reopen_closed_tab(),
            MenuAction::Dismiss => {}
        }
    }

    fn open_app_menu(&mut self) {
        let menu = menu_button(self.viewport_w);
        let zoom = (self.page().zoom * 100.0).round() as i32;
        let mut items = vec![
            ("New tab".to_string(), MenuAction::NewTabPage),
            ("New window".to_string(), MenuAction::NewWindow),
        ];
        if self.can_back() {
            items.push(("Back".to_string(), MenuAction::Back));
        }
        if self.can_forward() {
            items.push(("Forward".to_string(), MenuAction::Forward));
        }
        items.push(("Reload".to_string(), MenuAction::Reload));
        items.push(("Bookmark this page".to_string(), MenuAction::Bookmark));
        items.push(("Bookmarks".to_string(), MenuAction::Bookmarks));
        items.push(("History".to_string(), MenuAction::History));
        items.push(("Downloads".to_string(), MenuAction::Downloads));
        items.push(("Zoom in".to_string(), MenuAction::ZoomIn));
        items.push(("Zoom out".to_string(), MenuAction::ZoomOut));
        items.push((format!("Zoom {zoom}%"), MenuAction::ZoomReset));
        items.push(("Find".to_string(), MenuAction::Find));
        items.push(("Settings".to_string(), MenuAction::Settings));
        items.push((
            if self.dark {
                "Light theme".to_string()
            } else {
                "Dark theme".to_string()
            },
            MenuAction::Dark,
        ));
        self.menu = Some(Menu {
            x: menu.x,
            y: menu.bottom(),
            items,
        });
    }

    fn open_tab_search(&mut self) {
        let strip = self.tab_strip();
        let items = self
            .tabs
            .pages
            .iter()
            .enumerate()
            .map(|(index, page)| {
                let title = if page.title.is_empty() {
                    page.current.to_display()
                } else {
                    page.title.clone()
                };
                (title, MenuAction::Select(index))
            })
            .collect();
        self.menu = Some(Menu {
            x: strip.search.x,
            y: strip.search.bottom(),
            items,
        });
    }

    fn open_extensions_menu(&mut self) {
        let bar = toolbar(self.viewport_w);
        self.menu = Some(Menu {
            x: bar.extensions.x,
            y: bar.extensions.bottom(),
            items: vec![("No extensions".to_string(), MenuAction::Dismiss)],
        });
    }

    fn open_profile_menu(&mut self) {
        let bar = toolbar(self.viewport_w);
        self.menu = Some(Menu {
            x: bar.profile.x,
            y: bar.profile.bottom(),
            items: vec![
                ("Mithu profile".to_string(), MenuAction::Dismiss),
                (
                    if self.dark {
                        "Light theme".to_string()
                    } else {
                        "Dark theme".to_string()
                    },
                    MenuAction::Dark,
                ),
            ],
        });
    }

    fn open_site_info(&mut self) {
        let bar = toolbar(self.viewport_w);
        let label = match self.page().current.scheme.as_str() {
            "https" => "Connection is secure",
            "http" => "Connection is not secure",
            _ => "Mithu page",
        };
        self.menu = Some(Menu {
            x: bar.address.x,
            y: bar.address.bottom(),
            items: vec![(label.to_string(), MenuAction::Dismiss)],
        });
    }

    fn open_in_new_tab(&mut self, href: String) {
        let url = {
            let page = self.page();
            page.base
                .resolve(&href)
                .unwrap_or_else(|_| interpret(&href))
        };
        self.omnibox_focused = false;
        self.find_focused = false;
        self.suggestion = None;
        self.bookmarks_open = false;
        self.tabs.pages.push(Page::blank(self.viewport_w));
        self.tabs.active = self.tabs.pages.len() - 1;
        self.navigate_url(url);
    }

    pub fn field_focused(&self) -> bool {
        self.page().focused.is_some() && !self.omnibox_focused
    }

    pub fn field_insert(&mut self, ch: char) {
        let Some(id) = self.page().focused else {
            return;
        };
        if let Some(session) = self.page_mut().session.as_mut() {
            session.edit(|doc| {
                let mut text = field_text(doc, id);
                text.push(ch);
                set_field_text(doc, id, &text);
            });
            session.dispatch(id, "input");
        }
        self.pull_page();
    }

    pub fn field_backspace(&mut self) {
        let Some(id) = self.page().focused else {
            return;
        };
        if let Some(session) = self.page_mut().session.as_mut() {
            session.edit(|doc| {
                let mut text = field_text(doc, id);
                text.pop();
                set_field_text(doc, id, &text);
            });
            session.dispatch(id, "input");
        }
        self.pull_page();
    }

    pub fn submit_field(&mut self) {
        let Some(id) = self.page().focused else {
            return;
        };
        let submission = self
            .page_mut()
            .session
            .as_mut()
            .and_then(|session| session.submission(id));
        self.pull_page();
        if let Some(submission) = submission {
            self.send_form(submission);
        }
    }

    pub fn blur_page(&mut self) {
        self.omnibox_focused = false;
        self.suggestion = None;
        let page = self.page_mut();
        page.select_all = false;
        page.focused = None;
    }

    pub fn open_find(&mut self) {
        self.omnibox_focused = false;
        self.find_focused = true;
        self.page_mut().find_open = true;
        self.reveal_find();
    }

    pub fn close_find(&mut self) {
        self.find_focused = false;
        self.page_mut().find_open = false;
    }

    pub fn find_insert(&mut self, ch: char) {
        if ch.is_control() {
            return;
        }
        {
            let page = self.page_mut();
            page.find_query.push(ch);
            page.find_at = 0;
            page.find_open = true;
        }
        self.find_focused = true;
        self.reveal_find();
    }

    pub fn find_backspace(&mut self) {
        {
            let page = self.page_mut();
            page.find_query.pop();
            page.find_at = 0;
        }
        self.reveal_find();
    }

    pub fn find_next(&mut self) {
        self.step_find(1);
    }

    pub fn find_previous(&mut self) {
        self.step_find(-1);
    }

    pub fn find_is_open(&self) -> bool {
        self.page().find_open
    }

    pub fn find_count(&self) -> usize {
        self.find_hits().len()
    }

    pub fn find_active(&self) -> Option<usize> {
        let count = self.find_hits().len();
        (count > 0).then(|| self.page().find_at.min(count - 1))
    }

    fn step_find(&mut self, delta: i32) {
        let count = self.find_hits().len() as i32;
        if count == 0 {
            return;
        }
        let at = self.page().find_at as i32;
        let next = (at + delta).rem_euclid(count) as usize;
        self.page_mut().find_at = next;
        self.reveal_find();
    }

    fn reveal_find(&mut self) {
        let hits = self.find_hits();
        if hits.is_empty() {
            return;
        }
        let at = self.page().find_at.min(hits.len() - 1);
        self.page_mut().find_at = at;
        let Some(rect) = hits[at].first().copied() else {
            return;
        };
        let view = self.page_height();
        let scroll = self.page().scroll;
        let next = if rect.y < scroll {
            (rect.y - 8.0).max(0.0)
        } else if rect.bottom() > scroll + view {
            (rect.bottom() - view + 8.0).max(0.0)
        } else {
            scroll
        };
        let max = (self.page().layout.content_height - view).max(0.0);
        self.page_mut().scroll = next.min(max);
    }

    fn find_hits(&self) -> Vec<Vec<Rect>> {
        let page = self.page();
        if !page.find_open || page.find_query.is_empty() {
            return Vec::new();
        }
        text_matches(&self.fonts, &page.layout, &page.find_query)
    }

    pub fn focus_omnibox(&mut self) {
        self.omnibox_focused = true;
        self.find_focused = false;
        self.bookmarks_open = false;
        self.suggestion = None;
        self.page_mut().select_all = true;
    }

    pub fn blur_omnibox(&mut self) {
        self.omnibox_focused = false;
        self.suggestion = None;
        self.page_mut().select_all = false;
    }

    pub fn insert_char(&mut self, ch: char) {
        if ch.is_control() {
            return;
        }
        self.suggestion = None;
        let page = self.page_mut();
        if page.select_all {
            page.omnibox.clear();
            page.cursor = 0;
            page.select_all = false;
        }
        let mut chars: Vec<char> = page.omnibox.chars().collect();
        let index = page.cursor.min(chars.len());
        chars.insert(index, ch);
        page.cursor = index + 1;
        page.omnibox = chars.into_iter().collect();
    }

    pub fn backspace(&mut self) {
        self.suggestion = None;
        let page = self.page_mut();
        if page.select_all {
            page.omnibox.clear();
            page.cursor = 0;
            page.select_all = false;
            return;
        }
        if page.cursor == 0 {
            return;
        }
        let mut chars: Vec<char> = page.omnibox.chars().collect();
        let index = page.cursor.min(chars.len());
        if index == 0 {
            return;
        }
        chars.remove(index - 1);
        page.cursor = index - 1;
        page.omnibox = chars.into_iter().collect();
    }

    pub fn move_cursor(&mut self, delta: i32) {
        let page = self.page_mut();
        let len = page.omnibox.chars().count();
        if page.select_all {
            page.select_all = false;
            page.cursor = if delta < 0 { 0 } else { len };
            return;
        }
        page.cursor = (page.cursor as i32 + delta).clamp(0, len as i32) as usize;
    }

    pub fn select_all_text(&mut self) {
        self.page_mut().select_all = true;
    }

    pub fn move_suggestion(&mut self, delta: i32) {
        let count = self.suggestions().len() as i32;
        if count == 0 {
            return;
        }
        let current = self.suggestion.map(|index| index as i32).unwrap_or(-1);
        let next = (current + delta).clamp(0, count - 1) as usize;
        self.suggestion = Some(next);
    }

    pub fn submit_omnibox(&mut self) {
        if let Some(index) = self.suggestion
            && let Some(url) = self.suggestions().get(index).map(|item| item.url.clone())
        {
            self.omnibox_focused = false;
            self.suggestion = None;
            self.page_mut().select_all = false;
            self.navigate_url(url);
            return;
        }
        let text = self.page().omnibox.clone();
        self.omnibox_focused = false;
        self.suggestion = None;
        self.page_mut().select_all = false;
        self.navigate(&text);
    }

    fn suggestions(&self) -> Vec<Suggestion> {
        if !self.omnibox_focused {
            return Vec::new();
        }
        let query = self.page().omnibox.trim().to_string();
        if query.is_empty() {
            return Vec::new();
        }
        let mut items = Vec::new();
        let typed = interpret(&query);
        let typed_label = self
            .bookmarks
            .iter()
            .find(|item| item.url.to_display() == typed.to_display())
            .map(|item| item.title.clone())
            .unwrap_or_else(|| typed.to_display());
        items.push(Suggestion {
            label: typed_label,
            url: typed,
            kind: SuggestionKind::Url,
        });
        let needle = query.to_ascii_lowercase();
        for bookmark in &self.bookmarks {
            if items.len() >= 8 {
                break;
            }
            let hay =
                format!("{} {}", bookmark.title, bookmark.url.to_display()).to_ascii_lowercase();
            if hay.contains(&needle)
                && items
                    .iter()
                    .all(|item| item.url.to_display() != bookmark.url.to_display())
            {
                items.push(Suggestion {
                    label: bookmark.title.clone(),
                    url: bookmark.url.clone(),
                    kind: SuggestionKind::Bookmark,
                });
            }
        }
        for url in self.page().history.iter().rev() {
            if items.len() >= 8 {
                break;
            }
            let display = url.to_display();
            if display.to_ascii_lowercase().contains(&needle)
                && items.iter().all(|item| item.url.to_display() != display)
            {
                items.push(Suggestion {
                    label: display,
                    url: url.clone(),
                    kind: SuggestionKind::History,
                });
            }
        }
        items
    }

    pub fn title(&self) -> &str {
        &self.page().title
    }

    pub fn location(&self) -> String {
        self.page().current.to_display()
    }

    pub fn render(&mut self, width: f32, height: f32) -> Vec<u32> {
        let width = width.max(1.0);
        let height = height.max(1.0);
        if (self.viewport_w - width).abs() > 0.5 || (self.viewport_h - height).abs() > 0.5 {
            self.resize(width, height);
        }
        self.paint(width.round() as u32, height.round() as u32)
    }

    fn navigate_url(&mut self, url: Url) {
        let same = {
            let page = self.page();
            page.history.get(page.index) == Some(&url)
        };
        if same {
            self.load_url(url, true);
            return;
        }
        {
            let page = self.page_mut();
            if page.index + 1 < page.history.len() {
                page.history.truncate(page.index + 1);
            }
            page.history.push(url.clone());
            page.index = page.history.len() - 1;
        }
        self.load_url(url, false);
    }

    fn drag_tab(&mut self, from: usize, x: f32) {
        let tabs = self.tab_strip().tabs;
        let Some(to) = tabs.iter().position(|rect| x < rect.right()) else {
            return;
        };
        if to == from || to >= self.tabs.pages.len() {
            return;
        }
        let page = self.tabs.pages.remove(from);
        self.tabs.pages.insert(to, page);
        self.tabs.active = match self.tabs.active {
            active if active == from => to,
            active if from < active && to >= active => active - 1,
            active if from > active && to <= active => active + 1,
            active => active,
        };
        self.drag = Some(to);
    }

    fn load_url(&mut self, url: Url, bypass_cache: bool) {
        self.page_mut().status = format!("Loading {}", url.to_display());
        if url.scheme == "about" && matches!(url.path.as_str(), "history" | "downloads") {
            let html = if url.path == "history" {
                history_page(&self.visits)
            } else {
                downloads_page(&self.downloads)
            };
            let status = if url.path == "history" {
                "History".to_string()
            } else {
                "Downloads".to_string()
            };
            self.install(Fetched {
                url,
                html,
                status,
                download_name: None,
            });
            return;
        }
        let initiator = {
            let host = self.page().current.host.clone();
            if host.is_empty() { None } else { Some(host) }
        };
        match retrieve(&url, bypass_cache, initiator.as_deref()) {
            Ok(fetched) => self.install(fetched),
            Err(message) => self.install(Fetched {
                html: error_page(&url, &message),
                status: message,
                url,
                download_name: None,
            }),
        }
    }

    fn install(&mut self, mut fetched: Fetched) {
        // SpiderMonkey keeps one context per thread. The previous page's
        // context has to be destroyed before this page creates its own.
        self.page_mut().session = None;
        if let Some(name) = fetched.download_name.clone() {
            self.downloads.push((name, fetched.url.to_display()));
            if self.downloads.len() > 100 {
                self.downloads.remove(0);
            }
            fetched.html = downloads_page(&self.downloads);
            fetched.status = "Downloads".to_string();
            fetched.url = Url::parse("about:downloads").expect("downloads");
        }
        let find_query = self.page().find_query.clone();
        let find_open = self.page().find_open;
        let zoom = self.page().zoom;
        let layout_width = self.layout_width();
        let document = parse_html(&fetched.html);
        let base = match &document.base_href {
            Some(href) => fetched
                .url
                .resolve(href)
                .unwrap_or_else(|_| fetched.url.clone()),
            None => fetched.url.clone(),
        };
        let timed = std::env::var_os("MITHU_TIMING").is_some();
        let mark = Instant::now();
        prefetch_page(&document, &base);
        if timed {
            eprintln!("timing prefetch {}ms", mark.elapsed().as_millis());
        }
        let module_base = base.clone();
        let origin = origin_key(&fetched.url);
        let local = self.local_data.get(&origin).cloned().unwrap_or_default();
        let session_store = self.session_data.get(&origin).cloned().unwrap_or_default();
        let mut session = Session::open_cooperative(
            document,
            &fetched.url.to_display(),
            move |src| fetch_text(&module_base, src),
            local,
            session_store,
        );
        if timed {
            eprintln!("timing session {}ms", mark.elapsed().as_millis());
        }
        if !self.cooperative {
            while session.scripts_pending() {
                session.pump(Duration::from_secs(60));
            }
        }
        let scripts_pending = session.scripts_pending();
        self.local_data
            .insert(origin.clone(), session.local_storage());
        self.session_data.insert(origin, session.session_storage());
        if !scripts_pending {
            if let Some(next) = session.take_navigation() {
                if self.js_hops < 8 {
                    self.js_hops += 1;
                    self.navigate(&next);
                    return;
                }
                self.js_hops = 0;
            } else {
                self.js_hops = 0;
            }
        }
        let logs = session.logs();
        session.edit(|doc| doc.refresh_metadata());
        let document = session.document();
        let mut author = String::new();
        for sheet in &document.stylesheets {
            author.push_str(sheet);
            author.push('\n');
        }
        let mut seen = Vec::new();
        if timed {
            eprintln!("timing css-start {}ms", mark.elapsed().as_millis());
        }
        for href in document.stylesheet_links.iter().take(40) {
            if let Some(css) = fetch_text(&base, href)
                && css.len() <= 1_500_000
            {
                author.push_str(&expand_imports(&css, &base, 0, &mut seen));
                author.push('\n');
            }
        }
        author = expand_imports(&author, &base, 0, &mut seen);
        if timed {
            eprintln!(
                "timing css {}ms bytes={}",
                mark.elapsed().as_millis(),
                author.len()
            );
        }
        self.install_web_fonts(&author, &base);
        if timed {
            eprintln!("timing fonts {}ms", mark.elapsed().as_millis());
        }
        let styles = style_with(
            &document,
            &author,
            StyleState {
                hover: None,
                viewport_width: layout_width,
                ..StyleState::default()
            },
        );
        if timed {
            eprintln!("timing style {}ms nodes={}", mark.elapsed().as_millis(), document.nodes.len());
        }
        let images = load_images(&document, &base, &styles);
        if timed {
            eprintln!("timing images {}ms", mark.elapsed().as_millis());
        }
        let layout_height = self.layout_height();
        let layout = layout_tree(
            &document,
            &styles,
            layout_width,
            layout_height,
            &self.fonts,
            &images,
        );
        session.set_geometry(Self::metrics_from(&layout.root), 0.0);
        if timed {
            eprintln!("timing layout {}ms", mark.elapsed().as_millis());
        }
        let painted_revision = Some(session.revision());
        self.omnibox_focused = false;
        let page = self.page_mut();
        let committed = process::RendererToBrowser::Committed {
            url: fetched.url.to_display(),
            title: if document.title.is_empty() {
                fetched.url.to_display()
            } else {
                document.title.clone()
            },
        };
        let process::RendererToBrowser::Committed { title, .. } = &committed else {
            return;
        };
        page.title = title.clone();
        page.status = if scripts_pending {
            "Loading".to_string()
        } else if logs.is_empty() {
            fetched.status
        } else {
            logs.last().cloned().unwrap_or(fetched.status)
        };
        page.current = fetched.url.clone();
        page.base = base;
        page.omnibox = fetched.url.to_display();
        page.cursor = page.omnibox.chars().count();
        page.select_all = false;
        page.scroll = 0.0;
        page.document = document;
        page.styles = styles;
        page.images = images;
        page.layout = layout;
        page.session = Some(session);
        page.focused = None;
        // React and Polymer schedule paints on MessageChannel. Those tasks are
        // due immediately, so drain them before the first painted frame.
        self.advance_timers(0);
        let page = self.page_mut();
        page.find_query = find_query;
        page.find_open = find_open;
        page.find_at = 0;
        page.zoom = zoom;
        page.author_css = author;
        page.hover = None;
        page.painted_revision = painted_revision;
        let reopen = page.find_open && !page.find_query.is_empty();
        let _ = page;
        self.publish_geometry();
        if reopen {
            self.reveal_find();
        }
        let visit = (self.page().title.clone(), self.page().current.to_display());
        if !visit.1.starts_with("about:history") && !visit.1.starts_with("about:downloads") {
            self.visits.push(visit);
            if self.visits.len() > 200 {
                self.visits.remove(0);
            }
        }
    }

    pub fn advance_timers(&mut self, ms: u64) -> bool {
        let (ran, next) = {
            let Some(session) = self.page_mut().session.as_mut() else {
                return false;
            };
            (session.advance(ms), session.take_navigation())
        };
        if ran
            || self
                .page()
                .session
                .as_ref()
                .is_some_and(|session| !session.logs().is_empty())
        {
            self.pull_page();
        }
        if let Some(next) = next {
            self.navigate(&next);
            return true;
        }
        ran
    }

    pub fn next_timer_ms(&self) -> Option<u64> {
        self.page()
            .session
            .as_ref()
            .and_then(|session| session.next_timer_ms())
    }

    fn layout_width(&self) -> f32 {
        (self.viewport_w / self.page().zoom.max(0.01)).max(1.0)
    }

    fn layout_height(&self) -> f32 {
        let find = if self.page().find_open { FIND_H } else { 0.0 };
        ((self.viewport_h - CONTENT_TOP - STATUS_H - PAGE_GAP - find) / self.page().zoom.max(0.01))
            .max(1.0)
    }

    fn relayout(&mut self) {
        let width = self.layout_width();
        let height = self.layout_height();
        let mut layout = {
            let page = self.tabs.page();
            layout_tree(
                &page.document,
                &page.styles,
                width,
                height,
                &self.fonts,
                &page.images,
            )
        };
        self.attach_frames(&mut layout);
        self.page_mut().layout = layout;
        self.publish_geometry();
    }

    fn publish_geometry(&mut self) {
        let boxes = Self::metrics_from(&self.page().layout.root);
        let scroll = self.page().scroll;
        if let Some(session) = self.page().session.as_ref() {
            session.set_geometry(boxes, scroll);
        }
    }

    fn metrics_from(box_: &LayoutBox) -> HashMap<usize, mithu_js::BoxMetrics> {
        let mut boxes = HashMap::new();
        fn walk(box_: &LayoutBox, boxes: &mut HashMap<usize, mithu_js::BoxMetrics>) {
            boxes.entry(box_.node.0).or_insert(mithu_js::BoxMetrics {
                x: box_.rect.x,
                y: box_.rect.y,
                width: box_.rect.width,
                height: box_.rect.height,
                border_top: box_.borders[0],
                border_right: box_.borders[1],
                border_bottom: box_.borders[2],
                border_left: box_.borders[3],
            });
            for child in &box_.children {
                walk(child, boxes);
            }
        }
        walk(box_, &mut boxes);
        boxes
    }

    /// A frame document gets its own realm. SpiderMonkey allows one context
    /// per thread, and the parent page already owns the UI thread's context,
    /// so the frame realm is created and destroyed on a worker thread.
    fn run_frame_scripts(doc: Document, base: &Url) -> Document {
        let module_base = base.clone();
        let display = base.to_display();
        std::thread::Builder::new()
            .name("mithu-frame".into())
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                let mut session = Session::open_cooperative(
                    doc,
                    &display,
                    move |src| fetch_text(&module_base, src),
                    HashMap::new(),
                    HashMap::new(),
                );
                let start = Instant::now();
                while session.scripts_pending() && start.elapsed() < Duration::from_millis(800) {
                    session.pump(Duration::from_millis(80));
                }
                session.document()
            })
            .expect("frame script thread")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }

    /// Load each iframe `src` into a nested document and paint it in the frame.
    fn attach_frames(&self, layout: &mut LayoutTree) {
        let base = self.page().base.clone();
        let page_url = self.page().current.to_display();
        let frames = self
            .page()
            .document
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, _node)| {
                let id = NodeId(index);
                if self.page().document.element_name(id) != Some("iframe") {
                    return None;
                }
                let src = self.page().document.attr(id, "src")?.to_string();
                if src.is_empty() || src.starts_with("about:") {
                    return None;
                }
                Some((id, src))
            })
            .collect::<Vec<_>>();
        for (id, src) in frames {
            let Some(html) = fetch_text(&base, &src) else {
                continue;
            };
            if html.len() > 500_000 {
                continue;
            }
            let resolved = base.resolve(&src).ok();
            if resolved
                .as_ref()
                .is_some_and(|url| url.to_display() == page_url)
            {
                continue;
            }
            let nested_base = resolved.unwrap_or_else(|| base.clone());
            let doc = parse_html(&html);
            let doc = Self::run_frame_scripts(doc, &nested_base);
            let author = author_css(&doc, &nested_base);
            let (width, height) = {
                let size = frame_size(&layout.root, id);
                (
                    if size.0 < 1.0 { 300.0 } else { size.0 },
                    if size.1 < 1.0 { 150.0 } else { size.1 },
                )
            };
            let styles = style_with(
                &doc,
                &author,
                StyleState {
                    hover: None,
                    viewport_width: width,
                    ..StyleState::default()
                },
            );
            let images = load_images(&doc, &nested_base, &styles);
            let nested = layout_tree(&doc, &styles, width, height, &self.fonts, &images);
            if let Some(slot) = find_box_mut(&mut layout.root, id) {
                slot.frame = Some(Box::new(nested));
            }
        }
    }

    pub fn pointer_up(&mut self) {
        self.drag = None;
    }

    pub fn pointer_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        self.hovering = true;
        if let Some(from) = self.drag {
            self.drag_tab(from, x);
            return;
        }
        if self.menu.is_some() || y < CONTENT_TOP || y >= self.viewport_h - STATUS_H {
            self.set_hover(None);
            return;
        }
        let scroll = self.page().scroll;
        let zoom = self.page().zoom.max(0.01);
        let doc_x = x / zoom;
        let doc_y = (y - CONTENT_TOP) / zoom + scroll;
        let hit = hit_test(&self.page().layout, doc_x, doc_y);
        self.set_hover(hit);
    }

    fn set_hover(&mut self, hit: Option<NodeId>) {
        if self.page().hover == hit {
            return;
        }
        self.page_mut().hover = hit;
        let hover = hit;
        let viewport_width = self.layout_width();
        let styles = {
            let page = self.tabs.page();
            style_with(
                &page.document,
                &page.author_css,
                StyleState {
                    hover,
                    viewport_width,
                    ..StyleState::default()
                },
            )
        };
        self.page_mut().styles = styles;
        self.relayout();
    }

    fn activate(&mut self, x: f32, y: f32) {
        let hit = hit_test(&self.page().layout, x, y);
        let mut allow_default = true;
        let mut focus_field = None;
        let mut submission = None;
        if let Some(node) = hit
            && let Some(session) = self.page_mut().session.as_mut()
        {
            allow_default = session.dispatch(node, "click");
            let document = session.document();
            if allow_default && let Some(field) = nearest(&document, node, is_text_field) {
                focus_field = Some(field);
            } else if allow_default && let Some(control) = nearest(&document, node, is_checkbox) {
                session.edit(|doc| toggle_checked(doc, control));
                session.dispatch(control, "change");
            } else if allow_default && let Some(control) = nearest(&document, node, is_submit) {
                submission = session.submission(control);
            }
        }
        self.pull_page();
        let next = self
            .page_mut()
            .session
            .as_mut()
            .and_then(|session| session.take_navigation());
        if let Some(next) = next {
            self.navigate(&next);
            return;
        }
        self.page_mut().focused = focus_field;
        if let Some(submission) = submission {
            self.send_form(submission);
            return;
        }
        if allow_default
            && focus_field.is_none()
            && let Some(href) = self.hit_link(x, y)
        {
            self.follow(href);
        }
    }

    fn install_web_fonts(&mut self, css: &str, base: &Url) {
        let wanted: Vec<(String, String)> = font_faces(css)
            .into_iter()
            .filter(|(family, _)| !self.fonts.has_family(family))
            .take(24)
            .collect();
        let hrefs = wanted.iter().map(|(_, url)| url.clone()).collect();
        let fetched = fetch_parallel(base, hrefs);
        for (family, url) in wanted {
            if let Some(bytes) = fetched.get(&url) {
                self.fonts.add_family(&family, bytes);
            }
        }
    }

    fn pull_page(&mut self) {
        let revision = self.page().session.as_ref().map(Session::revision);
        if revision.is_some() && revision == self.page().painted_revision {
            return;
        }
        let base = self.page().base.clone();
        let (document, logs) = {
            let Some(session) = self.page_mut().session.as_mut() else {
                return;
            };
            session.edit(|doc| doc.refresh_metadata());
            (session.document(), session.logs())
        };
        let author = author_css(&document, &base);
        self.install_web_fonts(&author, &base);
        let hover = self.page().hover;
        let viewport_width = self.layout_width();
        let styles = style_with(
            &document,
            &author,
            StyleState {
                hover,
                viewport_width,
                ..StyleState::default()
            },
        );
        let images = load_images(&document, &base, &styles);
        {
            let page = self.page_mut();
            if let Some(log) = logs.last() {
                page.status = log.clone();
            }
            if !document.title.is_empty() {
                page.title = document.title.clone();
            }
            page.styles = styles;
            page.images = images;
            page.document = document;
            page.author_css = author;
            page.painted_revision = revision;
        }
        self.relayout();
    }

    fn send_form(&mut self, submission: Submission) {
        let mut url = if submission.action.is_empty() {
            self.page().current.clone()
        } else {
            self.page()
                .base
                .resolve(&submission.action)
                .unwrap_or_else(|_| interpret(&submission.action))
        };
        if submission.method == "post" && url.is_network() {
            match post(
                &url,
                "application/x-www-form-urlencoded",
                submission.query.as_bytes(),
            ) {
                Ok(response) => {
                    let status = format!("{} {}", response.status, response.url.to_display());
                    let html =
                        bytes_to_html(&response.url, response.content_type(), &response.body);
                    self.install(Fetched {
                        url: response.url,
                        html,
                        status,
                        download_name: None,
                    });
                }
                Err(error) => self.install(Fetched {
                    html: error_page(&url, &error.to_string()),
                    status: error.to_string(),
                    url,
                    download_name: None,
                }),
            }
            return;
        }
        if !submission.query.is_empty() {
            url.query = Some(match url.query.as_deref() {
                Some(existing) if !existing.is_empty() => {
                    format!("{existing}&{}", submission.query)
                }
                _ => submission.query,
            });
        }
        self.navigate_url(url);
    }

    fn follow(&mut self, href: String) {
        let url = self
            .page()
            .base
            .resolve(&href)
            .unwrap_or_else(|_| interpret(&href));
        self.navigate_url(url);
    }

    fn hit_link(&self, x: f32, y: f32) -> Option<String> {
        self.page()
            .layout
            .links
            .iter()
            .rev()
            .find(|link| link.rect.contains(x, y))
            .map(|link| link.href.clone())
    }

    fn can_back(&self) -> bool {
        self.page().index > 0
    }

    fn can_forward(&self) -> bool {
        self.page().index + 1 < self.page().history.len()
    }

    fn device_page_height(&self) -> f32 {
        let find = if self.page().find_open { FIND_H } else { 0.0 };
        (self.viewport_h - CONTENT_TOP - STATUS_H - PAGE_GAP - find).max(1.0)
    }

    fn page_height(&self) -> f32 {
        (self.device_page_height() / self.page().zoom.max(0.01)).max(1.0)
    }

    fn max_scroll(&self) -> f32 {
        (self.page().layout.content_height - self.page_height()).max(0.0)
    }

    fn paint(&self, width: u32, height: u32) -> Vec<u32> {
        let background = self.page().layout.background;
        let mut pixels = vec![background.to_u32(); width as usize * height as usize];
        let zoom = self.page().zoom;
        if (zoom - 1.0).abs() < 0.001 {
            self.paint_page(&mut pixels, width, height, CONTENT_TOP);
        } else {
            self.paint_zoomed_page(&mut pixels, width, height, zoom);
        }
        let mut canvas = Canvas {
            pixels: &mut pixels,
            width: width as i32,
            height: height as i32,
        };
        self.paint_chrome(&mut canvas);
        pixels
    }

    fn paint_page(&self, pixels: &mut [u32], width: u32, height: u32, origin_y: f32) {
        let scroll = self.page().scroll;
        let find = if self.page().find_open { FIND_H } else { 0.0 };
        let clip = Rect::new(
            0.0,
            origin_y,
            width as f32,
            (height as f32 - origin_y - STATUS_H - PAGE_GAP - find).max(1.0),
        );
        let mut canvas = Canvas {
            pixels,
            width: width as i32,
            height: height as i32,
        };
        self.paint_find_hits(&mut canvas, scroll, clip, origin_y);
        paint_layout(
            &mut canvas,
            &self.tabs.page().layout,
            &self.fonts,
            clip,
            scroll,
            origin_y,
        );
    }

    fn paint_zoomed_page(&self, pixels: &mut [u32], width: u32, height: u32, zoom: f32) {
        let scroll = self.page().scroll;
        let find = if self.page().find_open { FIND_H } else { 0.0 };
        let device_h = (height as f32 - CONTENT_TOP - STATUS_H - PAGE_GAP - find).max(1.0);
        let css_w = ((width as f32) / zoom).round().max(1.0) as i32;
        let css_h = (device_h / zoom).round().max(1.0) as i32;
        let mut page_pixels =
            vec![self.page().layout.background.to_u32(); (css_w * css_h) as usize];
        let clip = Rect::new(0.0, 0.0, css_w as f32, css_h as f32);
        {
            let mut canvas = Canvas {
                pixels: &mut page_pixels,
                width: css_w,
                height: css_h,
            };
            self.paint_find_hits(&mut canvas, scroll, clip, 0.0);
            paint_layout(
                &mut canvas,
                &self.tabs.page().layout,
                &self.fonts,
                clip,
                scroll,
                0.0,
            );
        }
        blit_nearest(
            pixels,
            width as i32,
            &page_pixels,
            css_w,
            css_h,
            Rect::new(0.0, CONTENT_TOP, width as f32, device_h),
        );
    }

    fn paint_chrome(&self, canvas: &mut Canvas<'_>) {
        let width = canvas.width as f32;
        let height = canvas.height as f32;
        let colors = chrome_colors(self.dark);
        let full = Rect::new(0.0, 0.0, width, height);
        canvas.fill(Rect::new(0.0, 0.0, width, TAB_H), colors.strip, full);
        canvas.fill(
            Rect::new(0.0, TAB_H, width, TOOLBAR_H),
            colors.toolbar,
            full,
        );
        canvas.fill(
            Rect::new(0.0, TAB_H + TOOLBAR_H, width, BOOKMARK_BAR_H),
            colors.toolbar,
            full,
        );
        self.paint_tabs(canvas, &colors);
        let bar = toolbar(width);
        self.paint_toolbar_button(
            canvas,
            bar.back,
            icons::Kind::Back,
            self.can_back(),
            &colors,
        );
        self.paint_toolbar_button(
            canvas,
            bar.forward,
            icons::Kind::Forward,
            self.can_forward(),
            &colors,
        );
        let loading = self.page().status.starts_with("Loading");
        self.paint_toolbar_button(
            canvas,
            bar.reload,
            if loading {
                icons::Kind::Close
            } else {
                icons::Kind::Reload
            },
            true,
            &colors,
        );
        self.paint_toolbar_button(canvas, bar.home, icons::Kind::Home, true, &colors);
        self.paint_bookmarks_bar(canvas, &colors);
        let radius = bar.address.height / 2.0;
        let border = if self.omnibox_focused {
            colors.focus
        } else {
            colors.omnibox
        };
        canvas.fill_round(bar.address, border, radius, full);
        let inset = if self.omnibox_focused { 2.0 } else { 0.0 };
        canvas.fill_round(
            Rect::new(
                bar.address.x + inset,
                bar.address.y + inset,
                bar.address.width - inset * 2.0,
                bar.address.height - inset * 2.0,
            ),
            if self.omnibox_focused {
                colors.address
            } else {
                colors.omnibox
            },
            radius,
            full,
        );
        let chip = security_chip(&self.page().current, self.omnibox_focused);
        let chip_box = chip_rect(bar.address, &chip, &self.fonts);
        self.paint_chip(canvas, chip_box, &chip, &colors);
        if self.is_bookmarked() {
            canvas.fill(bar.star, BOOKMARKED, full);
        }
        self.paint_toolbar_button(
            canvas,
            bar.star,
            if self.is_bookmarked() {
                icons::Kind::StarFilled
            } else {
                icons::Kind::Star
            },
            true,
            &colors,
        );
        self.paint_extensions(canvas, bar.extensions, &colors);
        self.paint_profile(canvas, bar.profile, &colors);
        self.paint_more(canvas, bar.menu, &colors);
        let text_left = if chip_box.width > 0.0 {
            chip_box.right() + 8.0
        } else {
            bar.address.x + 16.0
        };
        let text_clip = Rect::new(
            text_left,
            bar.address.y + 4.0,
            (bar.star.x - text_left - 4.0).max(1.0),
            bar.address.height - 8.0,
        );
        let address = bar.address;
        let size = 16.0;
        let prefix: String = self
            .page()
            .omnibox
            .chars()
            .take(self.page().cursor)
            .collect();
        let prefix_w = self.fonts.width(&prefix, size, false, false);
        let full_w = self.fonts.width(&self.page().omnibox, size, false, false);
        let mut shift = 0.0;
        if full_w > text_clip.width {
            shift = (prefix_w - text_clip.width + 12.0).max(0.0);
        }
        let text_x = text_clip.x - shift;
        let baseline = address.y + 22.0;
        if self.omnibox_focused && self.page().select_all && !self.page().omnibox.is_empty() {
            canvas.fill(
                Rect::new(text_x - 2.0, address.y + 6.0, full_w + 4.0, 22.0),
                SELECTION,
                text_clip,
            );
        }
        canvas.text(
            &self.fonts,
            text_x,
            baseline,
            &self.page().omnibox,
            size,
            colors.ink,
            false,
            false,
            false,
            text_clip,
        );
        if self.omnibox_focused && !self.page().select_all {
            canvas.fill(
                Rect::new(text_x + prefix_w, address.y + 8.0, 1.5, 18.0),
                colors.focus,
                text_clip,
            );
        }
        if let Some(shelf) = self.shelf_rect() {
            self.paint_shelf(canvas, shelf, &colors);
        }
        canvas.fill(
            Rect::new(0.0, height - STATUS_H, width, STATUS_H),
            colors.status_bg,
            full,
        );
        let mut status = if self.page().title.is_empty() {
            self.page().status.clone()
        } else {
            format!("{}  —  {}", self.page().title, self.page().status)
        };
        if (self.page().zoom - 1.0).abs() > 0.001 {
            status.push_str(&format!("  ·  {}%", (self.page().zoom * 100.0).round()));
        }
        canvas.text(
            &self.fonts,
            12.0,
            height - 8.0,
            &status,
            13.0,
            colors.status_text,
            false,
            false,
            false,
            Rect::new(8.0, height - STATUS_H, width - 16.0, STATUS_H),
        );
        self.paint_tooltip(canvas, &colors);
        if self.page().find_open {
            self.paint_find_bar(canvas);
        }
        if self.bookmarks_open {
            self.paint_bookmarks(canvas);
        }
        if self.omnibox_focused {
            self.paint_suggestions(canvas);
        }
        if let Some(menu) = self.menu.clone() {
            self.paint_menu(canvas, &menu);
        }
        if let Some((x, y, started)) = self.ripple {
            let t = (started.elapsed().as_secs_f32() / 0.35).clamp(0.0, 1.0);
            if t < 1.0 {
                let alpha = ((1.0 - t) * 70.0) as u8;
                canvas.fill_circle(x, y, 6.0 + t * 36.0, Color::rgba(11, 87, 208, alpha), full);
            }
        }
    }

    fn paint_menu(&self, canvas: &mut Canvas<'_>, menu: &Menu) {
        let bounds = menu_bounds(menu, canvas.width as f32, canvas.height as f32);
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        canvas.fill(bounds, SEPARATOR, full);
        for (index, (label, action)) in menu.items.iter().enumerate() {
            let row = Rect::new(
                bounds.x + 1.0,
                bounds.y + index as f32 * 32.0 + 1.0,
                bounds.width - 2.0,
                30.0,
            );
            canvas.fill(row, ADDRESS, full);
            if let Some(kind) = menu_icon(action) {
                icons::draw(
                    canvas,
                    kind,
                    Rect::new(row.x + 8.0, row.y + 7.0, 16.0, 16.0),
                    ICON,
                    full,
                );
            }
            canvas.text(
                &self.fonts,
                row.x + 32.0,
                row.y + 20.0,
                label,
                14.0,
                INK,
                false,
                false,
                false,
                row,
            );
        }
    }

    fn paint_suggestions(&self, canvas: &mut Canvas<'_>) {
        let items = self.suggestions();
        if items.is_empty() {
            return;
        }
        let panel = suggestion_panel(canvas.width as f32, items.len());
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        canvas.fill(panel, SEPARATOR, full);
        for (index, item) in items.iter().enumerate() {
            let row = Rect::new(
                panel.x + 1.0,
                panel.y + index as f32 * 32.0 + 1.0,
                panel.width - 2.0,
                30.0,
            );
            let fill = if self.suggestion == Some(index) {
                SELECTION
            } else {
                ADDRESS
            };
            canvas.fill(row, fill, full);
            let glyph = Rect::new(row.x + 8.0, row.y + 7.0, 16.0, 16.0);
            let kind = match item.kind {
                SuggestionKind::Bookmark => icons::Kind::Star,
                SuggestionKind::History => icons::Kind::History,
                SuggestionKind::Url => icons::Kind::Search,
            };
            icons::draw(canvas, kind, glyph, ICON, full);
            canvas.text(
                &self.fonts,
                row.x + 32.0,
                row.y + 20.0,
                &item.label,
                14.0,
                INK,
                false,
                false,
                false,
                row,
            );
        }
    }

    fn paint_bookmarks(&self, canvas: &mut Canvas<'_>) {
        let width = canvas.width as f32;
        let panel = bookmark_panel(width, self.bookmarks.len());
        let full = Rect::new(0.0, 0.0, width, canvas.height as f32);
        canvas.fill(panel, SEPARATOR, full);
        if self.bookmarks.is_empty() {
            canvas.text(
                &self.fonts,
                panel.x + 12.0,
                panel.y + 21.0,
                "No bookmarks",
                14.0,
                ICON,
                false,
                false,
                false,
                panel,
            );
            return;
        }
        for (index, bookmark) in self.bookmarks.iter().enumerate().take(8) {
            let row = Rect::new(
                panel.x + 1.0,
                panel.y + index as f32 * 32.0 + 1.0,
                panel.width - 2.0,
                30.0,
            );
            canvas.fill(row, ADDRESS, full);
            canvas.text(
                &self.fonts,
                row.x + 8.0,
                row.y + 20.0,
                &bookmark.title,
                14.0,
                INK,
                false,
                false,
                false,
                row,
            );
        }
    }

    fn paint_find_hits(&self, canvas: &mut Canvas<'_>, scroll: f32, clip: Rect, origin_y: f32) {
        let hits = self.find_hits();
        let active = self.page().find_at;
        for (index, hit) in hits.iter().enumerate() {
            let color = if index == active {
                FIND_ACTIVE
            } else {
                FIND_HIT
            };
            for rect in hit {
                canvas.fill(
                    Rect::new(rect.x, rect.y - scroll + origin_y, rect.width, rect.height),
                    color,
                    clip,
                );
            }
        }
    }

    fn paint_find_bar(&self, canvas: &mut Canvas<'_>) {
        let width = canvas.width as f32;
        let height = canvas.height as f32;
        let bar = find_bar_rect(width, height);
        let full = Rect::new(0.0, 0.0, width, height);
        let border = if self.find_focused {
            ADDRESS_FOCUS
        } else {
            ADDRESS_BORDER
        };
        canvas.fill(bar, border, full);
        canvas.fill(
            Rect::new(bar.x + 2.0, bar.y + 2.0, bar.width - 4.0, bar.height - 4.0),
            FIND_BAR,
            full,
        );
        let count = self.find_hits().len();
        let label = if self.page().find_query.is_empty() {
            "Find".to_string()
        } else if count == 0 {
            format!("{}   0/0", self.page().find_query)
        } else {
            format!(
                "{}   {}/{count}",
                self.page().find_query,
                self.page().find_at.min(count - 1) + 1
            )
        };
        canvas.text(
            &self.fonts,
            bar.x + 12.0,
            bar.y + 24.0,
            &label,
            14.0,
            Color::rgb(28, 26, 22),
            false,
            false,
            false,
            Rect::new(bar.x + 8.0, bar.y + 4.0, bar.width - 16.0, bar.height - 8.0),
        );
    }

    fn paint_bookmarks_bar(&self, canvas: &mut Canvas<'_>, colors: &ChromeColors) {
        let width = canvas.width as f32;
        let y = TAB_H + TOOLBAR_H;
        let full = Rect::new(0.0, 0.0, width, canvas.height as f32);
        canvas.fill(
            Rect::new(0.0, y + BOOKMARK_BAR_H - 1.0, width, 1.0),
            colors.chip,
            full,
        );
        if self.bookmarks.is_empty() {
            canvas.text(
                &self.fonts,
                12.0,
                y + 22.0,
                "Bookmarks",
                13.0,
                colors.icon_disabled,
                false,
                false,
                false,
                Rect::new(8.0, y, width - 16.0, BOOKMARK_BAR_H),
            );
            return;
        }
        let slots = bookmark_slots(width, self.bookmarks.len());
        for (slot, bookmark) in slots.iter().zip(self.bookmarks.iter()) {
            let fill = if self.hovering && slot.contains(self.cursor.0, self.cursor.1) {
                colors.hover
            } else {
                colors.chip
            };
            canvas.fill_round(*slot, fill, 14.0, full);
            canvas.text(
                &self.fonts,
                slot.x + 10.0,
                slot.y + 18.0,
                &bookmark.title,
                13.0,
                colors.ink,
                false,
                false,
                false,
                *slot,
            );
        }
    }

    fn paint_tabs(&self, canvas: &mut Canvas<'_>, colors: &ChromeColors) {
        let strip = self.tab_strip();
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        for (index, rect) in strip.tabs.iter().enumerate() {
            let page = &self.tabs.pages[index];
            let active = index == self.tabs.active;
            let hovered = self.hovering && rect.contains(self.cursor.0, self.cursor.1);
            if active {
                // HorizontalTabStyleViews::GetPath for the active tab: top
                // corners curve in, and the bottom corners scoop out to the
                // toolbar so the tab and the toolbar are one shape.
                canvas.fill_contours(&[active_tab_contour(*rect)], colors.active, full, false);
            } else if hovered {
                canvas.fill_round(*rect, colors.hover, 10.0, full);
            } else if index > 0 && index != self.tabs.active && index - 1 != self.tabs.active {
                canvas.fill(
                    Rect::new(rect.x, rect.y + (rect.height - 20.0) / 2.0, 1.0, 20.0),
                    colors.hover,
                    full,
                );
            }
            if let Some(group) = page.group {
                let color = GROUP_COLORS[group as usize % GROUP_COLORS.len()];
                canvas.fill(
                    Rect::new(rect.x + 8.0, rect.bottom() - 3.0, rect.width - 16.0, 3.0),
                    color,
                    full,
                );
            }
            let favicon = Rect::new(
                rect.x + 8.0,
                rect.y + (rect.height - 16.0) / 2.0,
                16.0,
                16.0,
            );
            if page.status.starts_with("Loading") {
                self.paint_throbber(canvas, favicon, colors);
            } else {
                let letter = page
                    .title
                    .chars()
                    .next()
                    .or_else(|| page.current.host.chars().next())
                    .unwrap_or('M');
                canvas.fill_circle(favicon.x + 8.0, favicon.y + 8.0, 8.0, colors.chip, full);
                let text = letter.to_string();
                let width = self.fonts.width(&text, 11.0, true, false);
                canvas.text(
                    &self.fonts,
                    favicon.x + (16.0 - width) / 2.0,
                    favicon.y + 12.0,
                    &text,
                    11.0,
                    colors.ink,
                    true,
                    false,
                    false,
                    favicon,
                );
            }
            if !page.pinned {
                let title = if page.title.is_empty() {
                    page.current.to_display()
                } else {
                    page.title.clone()
                };
                let color = if active { colors.ink } else { colors.muted };
                let text_right = if strip.closes[index].width > 0.0 && (active || hovered) {
                    strip.closes[index].x - 4.0
                } else {
                    rect.right() - 8.0
                };
                canvas.text(
                    &self.fonts,
                    rect.x + 28.0,
                    rect.y + 22.0,
                    &title,
                    13.0,
                    color,
                    active,
                    false,
                    false,
                    Rect::new(
                        rect.x + 28.0,
                        rect.y,
                        (text_right - rect.x - 28.0).max(1.0),
                        rect.height,
                    ),
                );
            }
            if (active || hovered) && !page.pinned {
                icons::draw(
                    canvas,
                    icons::Kind::CloseTab,
                    strip.closes[index],
                    colors.icon,
                    full,
                );
            }
        }
        self.paint_round_icon(canvas, strip.plus, icons::Kind::Add, colors);
        self.paint_round_icon(canvas, strip.search, icons::Kind::Search, colors);
        self.paint_caption(canvas, colors);
    }

    fn paint_caption(&self, canvas: &mut Canvas<'_>, colors: &ChromeColors) {
        let [minimize, maximize, close] = caption_rects(canvas.width as f32);
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        for (rect, kind) in [
            (minimize, icons::Kind::Minimize),
            (maximize, icons::Kind::Maximize),
            (close, icons::Kind::Close),
        ] {
            let hovered = self.hovering && rect.contains(self.cursor.0, self.cursor.1);
            if hovered {
                let fill = if kind == icons::Kind::Close {
                    Color::rgb(196, 43, 28)
                } else {
                    colors.hover
                };
                canvas.fill(rect, fill, full);
            }
            let glyph = Rect::new(
                rect.x + (rect.width - 16.0) / 2.0,
                rect.y + (rect.height - 16.0) / 2.0,
                16.0,
                16.0,
            );
            let ink = if hovered && kind == icons::Kind::Close {
                Color::WHITE
            } else {
                colors.icon
            };
            icons::draw(canvas, kind, glyph, ink, full);
        }
    }

    fn paint_round_icon(
        &self,
        canvas: &mut Canvas<'_>,
        rect: Rect,
        kind: icons::Kind,
        colors: &ChromeColors,
    ) {
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        let hovered = self.hovering && rect.contains(self.cursor.0, self.cursor.1);
        canvas.fill_round(
            rect,
            if hovered { colors.hover } else { colors.plus },
            rect.height / 2.0,
            full,
        );
        let glyph = Rect::new(
            rect.x + (rect.width - 16.0) / 2.0,
            rect.y + (rect.height - 16.0) / 2.0,
            16.0,
            16.0,
        );
        icons::draw(canvas, kind, glyph, colors.icon, full);
    }

    fn paint_toolbar_button(
        &self,
        canvas: &mut Canvas<'_>,
        rect: Rect,
        kind: icons::Kind,
        enabled: bool,
        colors: &ChromeColors,
    ) {
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        let hovered = enabled && self.hovering && rect.contains(self.cursor.0, self.cursor.1);
        let pressed = self.ripple.is_some_and(|(x, y, started)| {
            rect.contains(x, y) && started.elapsed().as_millis() < 280
        });
        if hovered || pressed {
            canvas.fill_circle(
                rect.x + rect.width / 2.0,
                rect.y + rect.height / 2.0,
                16.0,
                if pressed {
                    Color::rgba(31, 31, 31, 36)
                } else {
                    Color::rgba(31, 31, 31, 18)
                },
                full,
            );
        }
        let color = if !enabled {
            colors.icon_disabled
        } else if kind == icons::Kind::StarFilled {
            STAR
        } else {
            colors.icon
        };
        let glyph = Rect::new(
            rect.x + (rect.width - 16.0) / 2.0,
            rect.y + (rect.height - 16.0) / 2.0,
            16.0,
            16.0,
        );
        icons::draw(canvas, kind, glyph, color, full);
    }

    fn paint_more(&self, canvas: &mut Canvas<'_>, rect: Rect, colors: &ChromeColors) {
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        let hovered = self.hovering && rect.contains(self.cursor.0, self.cursor.1);
        if hovered {
            canvas.fill_circle(
                rect.x + rect.width / 2.0,
                rect.y + rect.height / 2.0,
                16.0,
                Color::rgba(31, 31, 31, 18),
                full,
            );
        }
        let cx = rect.x + rect.width / 2.0;
        let cy = rect.y + rect.height / 2.0;
        for offset in [-5.0, 0.0, 5.0] {
            canvas.fill_circle(cx, cy + offset, 1.6, colors.icon, full);
        }
    }

    fn paint_extensions(&self, canvas: &mut Canvas<'_>, rect: Rect, colors: &ChromeColors) {
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        let hovered = self.hovering && rect.contains(self.cursor.0, self.cursor.1);
        if hovered {
            canvas.fill_circle(
                rect.x + rect.width / 2.0,
                rect.y + rect.height / 2.0,
                16.0,
                Color::rgba(31, 31, 31, 18),
                full,
            );
        }
        let glyph = Rect::new(
            rect.x + (rect.width - 16.0) / 2.0,
            rect.y + (rect.height - 16.0) / 2.0,
            16.0,
            16.0,
        );
        canvas.fill_round(glyph, colors.icon, 3.0, full);
        canvas.fill_round(
            Rect::new(glyph.x + 3.0, glyph.y + 3.0, 10.0, 10.0),
            colors.toolbar,
            2.0,
            full,
        );
        canvas.fill(
            Rect::new(glyph.x + 7.0, glyph.y + 3.0, 2.0, 10.0),
            colors.icon,
            full,
        );
        canvas.fill(
            Rect::new(glyph.x + 3.0, glyph.y + 7.0, 10.0, 2.0),
            colors.icon,
            full,
        );
    }

    fn paint_profile(&self, canvas: &mut Canvas<'_>, rect: Rect, colors: &ChromeColors) {
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        let cx = rect.x + rect.width / 2.0;
        let cy = rect.y + rect.height / 2.0;
        canvas.fill_circle(cx, cy, 12.0, colors.chip, full);
        canvas.fill_circle(cx, cy, 11.0, colors.focus, full);
        let width = self.fonts.width("M", 14.0, true, false);
        canvas.text(
            &self.fonts,
            cx - width / 2.0,
            cy + 5.0,
            "M",
            14.0,
            Color::WHITE,
            true,
            false,
            false,
            rect,
        );
    }

    fn paint_chip(&self, canvas: &mut Canvas<'_>, rect: Rect, chip: &Chip, colors: &ChromeColors) {
        if rect.width <= 0.0 {
            return;
        }
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        match chip {
            Chip::Hidden => {}
            Chip::Lock => {
                icons::draw(canvas, icons::Kind::Lock, rect, colors.icon, full);
            }
            Chip::NotSecure => {
                canvas.text(
                    &self.fonts,
                    rect.x,
                    rect.y + rect.height - 8.0,
                    "Not secure",
                    13.0,
                    colors.muted,
                    false,
                    false,
                    false,
                    rect,
                );
            }
            Chip::Name => {
                canvas.text(
                    &self.fonts,
                    rect.x,
                    rect.y + rect.height - 8.0,
                    "Mithu",
                    13.0,
                    colors.ink,
                    true,
                    false,
                    false,
                    rect,
                );
            }
        }
    }

    fn paint_throbber(&self, canvas: &mut Canvas<'_>, rect: Rect, colors: &ChromeColors) {
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        let cx = rect.x + rect.width / 2.0;
        let cy = rect.y + rect.height / 2.0;
        let shift = (self.opened.elapsed().as_millis() / 80) as usize;
        for index in 0..8 {
            let angle = index as f32 / 8.0 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            let color = if (index + shift) % 8 < 3 {
                colors.focus
            } else {
                colors.icon_disabled
            };
            canvas.fill_circle(
                cx + angle.cos() * 5.0,
                cy + angle.sin() * 5.0,
                1.3,
                color,
                full,
            );
        }
    }

    fn paint_shelf(&self, canvas: &mut Canvas<'_>, shelf: Rect, colors: &ChromeColors) {
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        canvas.fill_round(shelf, colors.menu, 12.0, full);
        canvas.fill(
            Rect::new(shelf.x, shelf.y, shelf.width, 1.0),
            colors.chip,
            full,
        );
        let glyph = Rect::new(
            shelf.x + 12.0,
            shelf.y + (shelf.height - 16.0) / 2.0,
            16.0,
            16.0,
        );
        icons::draw(canvas, icons::Kind::Download, glyph, colors.icon, full);
        let (name, url) = self.downloads.last().cloned().unwrap_or_default();
        canvas.text(
            &self.fonts,
            shelf.x + 36.0,
            shelf.y + 20.0,
            &name,
            13.0,
            colors.ink,
            true,
            false,
            false,
            shelf,
        );
        canvas.text(
            &self.fonts,
            shelf.x + 36.0,
            shelf.y + 36.0,
            &url,
            12.0,
            colors.muted,
            false,
            false,
            false,
            Rect::new(shelf.x + 36.0, shelf.y, shelf.width - 48.0, shelf.height),
        );
    }

    fn paint_tooltip(&self, canvas: &mut Canvas<'_>, colors: &ChromeColors) {
        if !self.hovering || self.menu.is_some() {
            return;
        }
        let Some(label) = self.tooltip_label() else {
            return;
        };
        let width = self.fonts.width(label, 12.0, false, false) + 16.0;
        let height = 24.0;
        let mut x = self.cursor.0 - width / 2.0;
        let mut y = self.cursor.1 + 18.0;
        let limit = canvas.width as f32;
        x = x.clamp(4.0, (limit - width - 4.0).max(4.0));
        if y + height > canvas.height as f32 - 4.0 {
            y = self.cursor.1 - height - 8.0;
        }
        let rect = Rect::new(x, y, width, height);
        let full = Rect::new(0.0, 0.0, limit, canvas.height as f32);
        canvas.fill_round(rect, colors.tooltip_bg, 4.0, full);
        canvas.text(
            &self.fonts,
            rect.x + 8.0,
            rect.y + 16.0,
            label,
            12.0,
            colors.tooltip_ink,
            false,
            false,
            false,
            rect,
        );
    }

    fn tooltip_label(&self) -> Option<&'static str> {
        let (x, y) = self.cursor;
        let strip = self.tab_strip();
        if strip.plus.contains(x, y) {
            return Some("New tab (Ctrl+T)");
        }
        if strip.search.contains(x, y) {
            return Some("Search tabs");
        }
        let bar = toolbar(self.viewport_w);
        if bar.back.contains(x, y) {
            return Some("Back (Alt+Left)");
        }
        if bar.forward.contains(x, y) {
            return Some("Forward (Alt+Right)");
        }
        if bar.reload.contains(x, y) {
            return Some("Reload (Ctrl+R)");
        }
        if bar.home.contains(x, y) {
            return Some("Home");
        }
        if bar.star.contains(x, y) {
            return Some("Bookmark this tab (Ctrl+D)");
        }
        if bar.extensions.contains(x, y) {
            return Some("Extensions");
        }
        if bar.profile.contains(x, y) {
            return Some("Profile");
        }
        if bar.menu.contains(x, y) {
            return Some("Customize and control Mithu");
        }
        None
    }

    fn shelf_rect(&self) -> Option<Rect> {
        if self.downloads.is_empty() {
            return None;
        }
        let bottom = if self.page().find_open {
            self.viewport_h - STATUS_H - FIND_H
        } else {
            self.viewport_h - STATUS_H
        };
        Some(Rect::new(8.0, bottom - 52.0, self.viewport_w - 16.0, 48.0))
    }

    fn tab_strip(&self) -> TabStrip {
        let pinned: Vec<bool> = self.tabs.pages.iter().map(|page| page.pinned).collect();
        tab_layout(self.viewport_w, &pinned)
    }
}

impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

fn blit_nearest(dst: &mut [u32], dst_w: i32, src: &[u32], src_w: i32, src_h: i32, dest: Rect) {
    if src_w <= 0 || src_h <= 0 || dest.width <= 0.0 || dest.height <= 0.0 || dst_w <= 0 {
        return;
    }
    let x0 = dest.x.floor().max(0.0) as i32;
    let y0 = dest.y.floor().max(0.0) as i32;
    let dst_h = dst.len() as i32 / dst_w;
    let x1 = dest.right().ceil().min(dst_w as f32) as i32;
    let y1 = dest.bottom().ceil().min(dst_h as f32) as i32;
    for y in y0..y1 {
        let v = (((y as f32 + 0.5 - dest.y) / dest.height) * src_h as f32).floor() as i32;
        let v = v.clamp(0, src_h - 1) as usize;
        let row = y as usize * dst_w as usize;
        for x in x0..x1 {
            let u = (((x as f32 + 0.5 - dest.x) / dest.width) * src_w as f32).floor() as i32;
            let u = u.clamp(0, src_w - 1) as usize;
            dst[row + x as usize] = src[v * src_w as usize + u];
        }
    }
}

fn find_bar_rect(width: f32, height: f32) -> Rect {
    Rect::new(0.0, height - STATUS_H - FIND_H, width, FIND_H)
}

struct TextRun {
    x: f32,
    baseline: f32,
    ascent: f32,
    height: f32,
    text: String,
    size: f32,
    bold: bool,
    mono: bool,
}

fn text_matches(fonts: &FontBook, tree: &LayoutTree, query: &str) -> Vec<Vec<Rect>> {
    let needle: Vec<char> = query.chars().map(|ch| ch.to_ascii_lowercase()).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut runs = Vec::new();
    collect_runs(&tree.root, &mut runs);
    let mut slots: Vec<Option<(usize, usize)>> = Vec::new();
    let mut folded = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        if index > 0
            && (run.baseline - runs[index - 1].baseline).abs() < 0.5
            && run.x > runs[index - 1].x + 0.5
        {
            folded.push(' ');
            slots.push(None);
        }
        for (char_index, ch) in run.text.chars().enumerate() {
            folded.push(ch.to_ascii_lowercase());
            slots.push(Some((index, char_index)));
        }
    }
    let mut hits = Vec::new();
    if folded.len() < needle.len() {
        return hits;
    }
    for start in 0..=folded.len() - needle.len() {
        if folded[start..start + needle.len()] != needle[..] {
            continue;
        }
        let mut rects = Vec::new();
        let mut cursor = start;
        let end = start + needle.len();
        while cursor < end {
            let Some((run_index, char_start)) = slots[cursor] else {
                cursor += 1;
                continue;
            };
            let mut char_end = char_start + 1;
            cursor += 1;
            while cursor < end
                && slots[cursor].is_some_and(|(next_run, next_char)| {
                    next_run == run_index && next_char == char_end
                })
            {
                char_end += 1;
                cursor += 1;
            }
            let run = &runs[run_index];
            let prefix: String = run.text.chars().take(char_start).collect();
            let slice: String = run
                .text
                .chars()
                .skip(char_start)
                .take(char_end - char_start)
                .collect();
            let x = run.x + fonts.width(&prefix, run.size, run.bold, run.mono);
            let width = fonts.width(&slice, run.size, run.bold, run.mono).max(1.0);
            rects.push(Rect::new(
                x,
                run.baseline - run.ascent,
                width,
                run.height.max(1.0),
            ));
        }
        if !rects.is_empty() {
            hits.push(rects);
        }
    }
    hits
}

fn collect_runs(box_: &mithu_layout::LayoutBox, runs: &mut Vec<TextRun>) {
    for line in &box_.lines {
        for fragment in &line.fragments {
            if fragment.text.is_empty() {
                continue;
            }
            runs.push(TextRun {
                x: fragment.x,
                baseline: line.baseline,
                ascent: line.ascent,
                height: line.height,
                text: fragment.text.clone(),
                size: fragment.size,
                bold: fragment.bold,
                mono: fragment.mono,
            });
        }
    }
    for child in &box_.children {
        collect_runs(child, runs);
    }
}

struct TabStrip {
    tabs: Vec<Rect>,
    closes: Vec<Rect>,
    plus: Rect,
    search: Rect,
}

#[cfg(test)]
fn tab_rects(width: f32, count: usize) -> (Vec<Rect>, Rect) {
    let pinned = vec![false; count.max(1)];
    let strip = tab_layout(width, &pinned);
    (strip.tabs, strip.plus)
}

fn tab_layout(width: f32, pinned: &[bool]) -> TabStrip {
    let count = pinned.len().max(1);
    let pinned_flags = if pinned.is_empty() {
        vec![false]
    } else {
        pinned.to_vec()
    };
    let y = 6.0;
    let height = 35.0;
    let reserved = 8.0 + 28.0 + 4.0 + 28.0 + 12.0 + CAPTION_W * 3.0;
    let available = (width - reserved).max(80.0);
    let pinned_count = pinned_flags.iter().filter(|flag| **flag).count() as f32;
    let pinned_w = 46.0;
    let normal_count = (count as f32 - pinned_count).max(0.0);
    let rest = (available - pinned_count * pinned_w).max(48.0);
    let normal_w = if normal_count == 0.0 {
        0.0
    } else {
        (rest / normal_count).clamp(48.0, 240.0)
    };
    let mut x = 8.0;
    let mut tabs = Vec::new();
    let mut closes = Vec::new();
    for flag in &pinned_flags {
        let tab_w = if *flag { pinned_w } else { normal_w };
        let rect = Rect::new(x, y, tab_w, height);
        let close = Rect::new(
            rect.right() - 8.0 - 16.0,
            rect.y + (rect.height - 16.0) / 2.0,
            16.0,
            16.0,
        );
        tabs.push(rect);
        closes.push(close);
        x += tab_w;
    }
    let plus = Rect::new(x + 6.0, y + 3.0, 28.0, 28.0);
    let search = Rect::new(plus.right() + 4.0, plus.y, 28.0, 28.0);
    TabStrip {
        tabs,
        closes,
        plus,
        search,
    }
}

struct Suggestion {
    label: String,
    url: Url,
    kind: SuggestionKind,
}

enum SuggestionKind {
    Url,
    History,
    Bookmark,
}

#[derive(Clone)]
enum MenuAction {
    Open(String),
    NewTab(String),
    Back,
    Forward,
    Reload,
    Bookmark,
    NewTabPage,
    Bookmarks,
    History,
    Downloads,
    NewWindow,
    Find,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Settings,
    Dark,
    CloseTab(usize),
    Pin(usize),
    Group(usize),
    Select(usize),
    Reopen,
    Dismiss,
}

#[derive(Clone)]
struct Menu {
    x: f32,
    y: f32,
    items: Vec<(String, MenuAction)>,
}

fn menu_icon(action: &MenuAction) -> Option<icons::Kind> {
    match action {
        MenuAction::NewTabPage | MenuAction::NewWindow => Some(icons::Kind::Add),
        MenuAction::Back => Some(icons::Kind::Back),
        MenuAction::Forward => Some(icons::Kind::Forward),
        MenuAction::Reload => Some(icons::Kind::Reload),
        MenuAction::Bookmark | MenuAction::Bookmarks => Some(icons::Kind::Star),
        MenuAction::History => Some(icons::Kind::History),
        MenuAction::Downloads => Some(icons::Kind::Download),
        MenuAction::Find => Some(icons::Kind::Find),
        MenuAction::Settings => Some(icons::Kind::Settings),
        MenuAction::CloseTab(_) => Some(icons::Kind::CloseTab),
        _ => None,
    }
}

fn menu_bounds(menu: &Menu, width: f32, height: f32) -> Rect {
    let menu_w = 280.0;
    let menu_h = (menu.items.len().max(1) as f32) * 32.0;
    let x = menu.x.min((width - menu_w - 4.0).max(4.0)).max(4.0);
    let y = menu.y.min((height - menu_h - 4.0).max(4.0)).max(4.0);
    Rect::new(x, y, menu_w, menu_h)
}

fn suggestion_panel(width: f32, count: usize) -> Rect {
    let (_, _, _, address) = chrome_rects(width);
    Rect::new(
        address.x,
        address.bottom(),
        address.width,
        count.max(1) as f32 * 32.0,
    )
}

fn toolbar_y() -> f32 {
    TAB_H + (TOOLBAR_H - BTN) / 2.0
}

struct Toolbar {
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

fn toolbar(width: f32) -> Toolbar {
    let y = toolbar_y();
    let mut x = 6.0;
    let back = Rect::new(x, y, BTN, BTN);
    x += BTN + 4.0;
    let forward = Rect::new(x, y, BTN, BTN);
    x += BTN + 4.0;
    let reload = Rect::new(x, y, BTN, BTN);
    x += BTN + 4.0;
    let home = Rect::new(x, y, BTN, BTN);
    x += BTN + 9.0;
    let trailing = BTN * 3.0 + 4.0 * 2.0 + 8.0;
    let address_w = (width - x - trailing).max(40.0);
    let address = Rect::new(x, y, address_w, BTN);
    let star = Rect::new(address.right() - BTN - 2.0, y, BTN, BTN);
    let mut right = address.right() + 8.0;
    let extensions = Rect::new(right, y, BTN, BTN);
    right += BTN + 4.0;
    let profile = Rect::new(right, y, BTN, BTN);
    right += BTN + 4.0;
    let menu = Rect::new(right, y, BTN, BTN);
    Toolbar {
        back,
        forward,
        reload,
        home,
        address,
        star,
        extensions,
        profile,
        menu,
    }
}

fn menu_button(width: f32) -> Rect {
    toolbar(width).menu
}

#[cfg(test)]
fn bookmark_buttons(width: f32) -> (Rect, Rect) {
    let bar = toolbar(width);
    (bar.star, bar.menu)
}

fn bookmark_slots(width: f32, count: usize) -> Vec<Rect> {
    let y = TAB_H + TOOLBAR_H + 3.0;
    let mut x = 8.0;
    let mut slots = Vec::new();
    for _ in 0..count.min(12) {
        let slot_w = 148.0;
        if x + slot_w > width - 8.0 {
            break;
        }
        slots.push(Rect::new(x, y, slot_w, 28.0));
        x += slot_w + 8.0;
    }
    slots
}

fn caption_rects(width: f32) -> [Rect; 3] {
    let close = Rect::new(width - CAPTION_W, 0.0, CAPTION_W, TAB_H);
    let maximize = Rect::new(width - CAPTION_W * 2.0, 0.0, CAPTION_W, TAB_H);
    let minimize = Rect::new(width - CAPTION_W * 3.0, 0.0, CAPTION_W, TAB_H);
    [minimize, maximize, close]
}

enum Chip {
    Lock,
    NotSecure,
    Name,
    Hidden,
}

fn security_chip(url: &Url, focused: bool) -> Chip {
    if focused {
        return Chip::Hidden;
    }
    match url.scheme.as_str() {
        "https" => Chip::Lock,
        "http" => Chip::NotSecure,
        _ => Chip::Name,
    }
}

fn chip_rect(address: Rect, chip: &Chip, fonts: &mithu_text::FontBook) -> Rect {
    let width = match chip {
        Chip::Hidden => {
            return Rect::new(address.x, address.y, 0.0, 0.0);
        }
        Chip::Lock => 16.0,
        Chip::NotSecure => fonts.width("Not secure", 13.0, false, false),
        Chip::Name => fonts.width("Mithu", 13.0, true, false),
    };
    let height = 16.0;
    Rect::new(
        address.x + 12.0,
        address.y + (address.height - height) / 2.0,
        width,
        height,
    )
}

fn bookmark_panel(width: f32, count: usize) -> Rect {
    let rows = count.clamp(1, 8) as f32;
    let panel_w = 280.0_f32.min((width - 16.0).max(80.0));
    Rect::new(width - panel_w - 8.0, CONTENT_TOP, panel_w, rows * 32.0)
}

fn chrome_rects(width: f32) -> (Rect, Rect, Rect, Rect) {
    let bar = toolbar(width);
    (bar.back, bar.forward, bar.reload, bar.address)
}

fn retrieve(url: &Url, bypass_cache: bool, initiator: Option<&str>) -> Result<Fetched, String> {
    match url.scheme.as_str() {
        "about" => {
            let (html, status) = match url.path.as_str() {
                "mithu" | "home" | "" => (home_page(), "Mithu".to_string()),
                "settings" => (settings_page(), "Settings".to_string()),
                "blank" => ("<!DOCTYPE html><html><head><title>about:blank</title></head><body></body></html>".to_string(), "about:blank".to_string()),
                other => (error_page(url, &format!("unknown about page {other}")), format!("unknown about:{other}")),
            };
            Ok(Fetched {
                url: url.clone(),
                html,
                status,
                download_name: None,
            })
        }
        "data" => {
            let html = decode_data_text(&url.path).unwrap_or_default();
            Ok(Fetched {
                url: url.clone(),
                html,
                status: "data".to_string(),
                download_name: None,
            })
        }
        "file" => {
            let path = url.path.clone();
            let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
            Ok(Fetched {
                url: url.clone(),
                html: bytes_to_html(url, Some("text/html"), &bytes),
                status: "file".to_string(),
                download_name: None,
            })
        }
        "http" | "https" => {
            let response = if bypass_cache {
                mithu_net::reload(url)
            } else if let Some(host) = initiator {
                mithu_net::navigate_from(url, host)
            } else {
                get(url)
            }
            .map_err(|error| error.to_string())?;
            if response.body.is_empty() && response.status >= 400 {
                return Err(format!("HTTP {}", response.status));
            }
            let status = format!("{} {}", response.status, response.url.to_display());
            let mime = response.content_type().unwrap_or("");
            let download_name = download_name(&response.url, mime);
            let html = if download_name.is_some() {
                String::new()
            } else {
                bytes_to_html(&response.url, response.content_type(), &response.body)
            };
            Ok(Fetched {
                url: response.url,
                html,
                status,
                download_name,
            })
        }
        other => Err(format!("Mithu cannot open {other} URLs")),
    }
}

fn frame_size(box_: &LayoutBox, id: NodeId) -> (f32, f32) {
    if box_.node == id && box_.rect.width > 1.0 && box_.rect.height > 1.0 {
        return (box_.rect.width, box_.rect.height);
    }
    for child in &box_.children {
        let size = frame_size(child, id);
        if size.0 > 1.0 {
            return size;
        }
    }
    if box_.node == id {
        return (box_.rect.width.max(300.0), box_.rect.height.max(150.0));
    }
    (0.0, 0.0)
}

fn find_box_mut(box_: &mut LayoutBox, id: NodeId) -> Option<&mut LayoutBox> {
    if box_.node == id {
        return Some(box_);
    }
    for child in &mut box_.children {
        if let Some(found) = find_box_mut(child, id) {
            return Some(found);
        }
    }
    None
}

/// Warm the disk cache for scripts, stylesheets, images, and preload links.
/// Eight connections run at once. Later loads of the same URL reuse the cache
/// instead of opening another TLS session.
fn prefetch_page(document: &Document, base: &Url) {
    let mut hrefs = Vec::new();
    for script in &document.scripts {
        if let Some(src) = &script.src {
            hrefs.push(src.clone());
        }
    }
    for href in &document.stylesheet_links {
        hrefs.push(href.clone());
    }
    for index in 0..document.nodes.len() {
        let id = NodeId(index);
        let Some(name) = document.element_name(id) else {
            continue;
        };
        if name == "link" {
            let rel = document.attr(id, "rel").unwrap_or("").to_ascii_lowercase();
            let preload = rel.split_whitespace().any(|part| {
                part == "preload" || part == "modulepreload" || part == "prefetch"
            });
            if preload && let Some(href) = document.attr(id, "href") {
                hrefs.push(href.to_string());
            }
        } else if name == "img"
            && hrefs.len() < 160
            && let Some(src) = document.attr(id, "src")
        {
            hrefs.push(src.to_string());
        }
    }
    let mut urls = Vec::new();
    for href in hrefs {
        if href.starts_with("data:") {
            continue;
        }
        let Ok(url) = base.resolve(&href) else {
            continue;
        };
        if url.scheme != "http" && url.scheme != "https" {
            continue;
        }
        if urls.iter().any(|seen: &Url| seen.to_display() == url.to_display()) {
            continue;
        }
        urls.push(url);
    }
    if urls.is_empty() {
        return;
    }
    let queue = std::sync::Mutex::new(urls);
    let workers = 8.min(queue.lock().map(|guard| guard.len()).unwrap_or(1)).max(1);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().ok().and_then(|mut guard| guard.pop());
                    let Some(url) = next else {
                        break;
                    };
                    let _ = get(&url);
                }
            });
        }
    });
}

fn author_css(document: &Document, base: &Url) -> String {
    let mut author = String::new();
    for sheet in &document.stylesheets {
        author.push_str(sheet);
        author.push('\n');
    }
    let mut seen = Vec::new();
    for href in document.stylesheet_links.iter().take(40) {
        if let Some(css) = fetch_text(base, href)
            && css.len() <= 1_500_000
        {
            author.push_str(&expand_imports(&css, base, 0, &mut seen));
            author.push('\n');
        }
    }
    expand_imports(&author, base, 0, &mut seen)
}

fn expand_imports(css: &str, base: &Url, depth: usize, seen: &mut Vec<String>) -> String {
    if depth > 3 {
        return css.to_string();
    }
    let mut out = String::new();
    let mut rest = css;
    loop {
        let trimmed = rest.trim_start();
        let offset = rest.len() - trimmed.len();
        if !trimmed.to_ascii_lowercase().starts_with("@import") {
            out.push_str(rest);
            break;
        }
        let after = &trimmed["@import".len()..];
        let Some((url, consumed)) = import_target(after) else {
            out.push_str(rest);
            break;
        };
        if !seen.iter().any(|item| item == &url) {
            seen.push(url.clone());
            if let Some(fetched) = fetch_text(base, &url) {
                let resolved = base.resolve(&url).unwrap_or_else(|_| base.clone());
                out.push_str(&expand_imports(&fetched, &resolved, depth + 1, seen));
                out.push('\n');
            }
        }
        let next = offset + "@import".len() + consumed;
        rest = if next < rest.len() { &rest[next..] } else { "" };
    }
    out
}

fn import_target(input: &str) -> Option<(String, usize)> {
    let trimmed = input.trim_start();
    let lead = input.len() - trimmed.len();
    let (body, quoted) = if trimmed.starts_with("url(") {
        (trimmed[4..].trim_start(), false)
    } else {
        (trimmed, true)
    };
    let quote = body.chars().next()?;
    let raw = if quote == '"' || quote == '\'' {
        let end = body[1..].find(quote)? + 1;
        body[1..end].to_string()
    } else if !quoted {
        let end = body.find(')')?;
        body[..end].trim().trim_matches(['"', '\'']).to_string()
    } else {
        return None;
    };
    let close = if trimmed.starts_with("url(") {
        trimmed.find(')')? + 1
    } else {
        let start = trimmed.find(quote)?;
        let end = trimmed[start + 1..].find(quote)? + start + 2;
        end
    };
    let after = &trimmed[close..];
    let semi = after.find(';')? + 1;
    Some((raw, lead + close + semi))
}

fn load_images(
    document: &Document,
    base: &Url,
    styles: &[mithu_style::ComputedStyle],
) -> HashMap<NodeId, Arc<Bitmap>> {
    let mut wanted: Vec<(NodeId, String)> = Vec::new();
    let mut network = Vec::new();
    for id in (0..document.nodes.len()).map(NodeId) {
        if wanted.len() >= 80 {
            break;
        }
        let name = document.element_name(id);
        if name != Some("img") && name != Some("video") {
            continue;
        }
        let src = if name == Some("video") {
            document.attr(id, "poster").map(str::to_string)
        } else {
            image_source(document, id)
        };
        let Some(src) = src else {
            continue;
        };
        if !src.starts_with("data:") {
            network.push(src.clone());
        }
        wanted.push((id, src));
    }
    for (index, style) in styles.iter().enumerate() {
        if wanted.len() >= 80 {
            break;
        }
        let Some(url) = style.background_image.as_deref() else {
            continue;
        };
        if url.contains("placeholder") {
            continue;
        }
        let id = NodeId(index);
        if wanted.iter().any(|(seen, _)| *seen == id) {
            continue;
        }
        if !url.starts_with("data:") {
            network.push(url.to_string());
        }
        wanted.push((id, url.to_string()));
    }
    let mut masks = 0;
    for (index, style) in styles.iter().enumerate() {
        if masks >= 48 {
            break;
        }
        let Some(url) = style.mask_image.as_deref() else {
            continue;
        };
        let id = NodeId(index);
        if wanted.iter().any(|(seen, _)| *seen == id) {
            continue;
        }
        if !url.starts_with("data:") {
            network.push(url.to_string());
        }
        wanted.push((id, url.to_string()));
        masks += 1;
    }
    let fetched = fetch_parallel(base, network);
    let mut images = HashMap::new();
    for (id, src) in wanted {
        let bitmap = cached_bitmap(&src, || {
            if src.starts_with("data:") {
                mithu_image::decode_data_url(&src).or_else(|| rasterize_svg_url(&src))
            } else {
                fetched.get(&src).and_then(|bytes| decode_image_bytes(bytes))
            }
        });
        if let Some(bitmap) = bitmap {
            images.insert(id, bitmap);
        }
    }
    images
}

fn bitmap_cache() -> &'static Mutex<HashMap<String, Arc<Bitmap>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Bitmap>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_bitmap(src: &str, decode: impl FnOnce() -> Option<Bitmap>) -> Option<Arc<Bitmap>> {
    if let Some(hit) = bitmap_cache().lock().ok().and_then(|map| map.get(src).cloned()) {
        return Some(hit);
    }
    let bitmap = Arc::new(decode()?);
    if let Ok(mut map) = bitmap_cache().lock() {
        if map.len() > 400 {
            map.clear();
        }
        map.insert(src.to_string(), Arc::clone(&bitmap));
    }
    Some(bitmap)
}

fn fetch_parallel(base: &Url, hrefs: Vec<String>) -> HashMap<String, Vec<u8>> {
    let mut unique = Vec::new();
    for href in hrefs {
        if !unique.iter().any(|seen: &String| seen == &href) {
            unique.push(href);
        }
    }
    if unique.is_empty() {
        return HashMap::new();
    }
    let queue = std::sync::Mutex::new(unique);
    let out = std::sync::Mutex::new(HashMap::new());
    let workers = 8
        .min(queue.lock().map(|guard| guard.len()).unwrap_or(1))
        .max(1);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().ok().and_then(|mut guard| guard.pop());
                    let Some(href) = next else {
                        break;
                    };
                    let Some(bytes) = fetch_bytes(base, &href) else {
                        continue;
                    };
                    if let Ok(mut map) = out.lock() {
                        map.insert(href, bytes);
                    }
                }
            });
        }
    });
    out.into_inner().unwrap_or_default()
}

fn nearest(
    document: &Document,
    id: NodeId,
    matches: fn(&Document, NodeId) -> bool,
) -> Option<NodeId> {
    let mut current = if document.element_name(id).is_some() {
        Some(id)
    } else {
        document.parent_element(id)
    };
    while let Some(node) = current {
        if matches(document, node) {
            return Some(node);
        }
        current = document.parent_element(node);
    }
    None
}

fn is_text_field(document: &Document, id: NodeId) -> bool {
    match document.element_name(id) {
        Some("textarea") => true,
        Some("input") => {
            let kind = document
                .attr(id, "type")
                .unwrap_or("text")
                .to_ascii_lowercase();
            matches!(
                kind.as_str(),
                "text" | "search" | "email" | "password" | "url" | "tel" | "number"
            )
        }
        _ => false,
    }
}

fn is_checkbox(document: &Document, id: NodeId) -> bool {
    document.element_name(id) == Some("input")
        && document
            .attr(id, "type")
            .is_some_and(|kind| matches!(kind.to_ascii_lowercase().as_str(), "checkbox" | "radio"))
}

fn is_submit(document: &Document, id: NodeId) -> bool {
    match document.element_name(id) {
        Some("button") => document
            .attr(id, "type")
            .unwrap_or("submit")
            .eq_ignore_ascii_case("submit"),
        Some("input") => document
            .attr(id, "type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("submit")),
        _ => false,
    }
}

fn toggle_checked(document: &mut Document, id: NodeId) {
    if document.attr(id, "checked").is_some() {
        document.remove_attr(id, "checked");
    } else {
        document.set_attr(id, "checked", "");
    }
}

fn field_text(document: &Document, id: NodeId) -> String {
    if document.element_name(id) == Some("textarea") {
        document.text_content(id)
    } else {
        document.attr(id, "value").unwrap_or("").to_string()
    }
}

fn set_field_text(document: &mut Document, id: NodeId, text: &str) {
    if document.element_name(id) == Some("textarea") {
        document.set_text_content(id, text);
    } else {
        document.set_attr(id, "value", text);
    }
}

fn image_source(document: &Document, id: NodeId) -> Option<String> {
    let src = document.attr(id, "src").map(str::to_string);
    let lazy = [
        "data-src",
        "data-original",
        "data-lazy-src",
        "data-bg",
        "data-image",
    ]
    .iter()
    .find_map(|name| document.attr(id, name).map(str::to_string));
    if let Some(lazy) = lazy.filter(|value| !value.is_empty()) {
        let placeholder = src.as_deref().is_none_or(|value| {
            value.contains("placeholder")
                || value.starts_with("data:image/gif")
                || value.starts_with("data:image/svg")
        });
        if placeholder {
            return Some(lazy);
        }
    }
    if let Some(srcset) = document.attr(id, "srcset").or_else(|| document.attr(id, "data-srcset"))
        && let Some(picked) = pick_srcset(srcset)
    {
        return Some(picked);
    }
    src
}

fn pick_srcset(value: &str) -> Option<String> {
    let mut best: Option<(f32, String)> = None;
    for candidate in value.split(',') {
        let mut parts = candidate.split_whitespace();
        let url = parts.next()?.to_string();
        if url.is_empty() {
            continue;
        }
        let descriptor = parts.next().unwrap_or("1x");
        let width = if let Some(number) = descriptor.strip_suffix('w') {
            number.parse::<f32>().unwrap_or(1280.0)
        } else if let Some(number) = descriptor.strip_suffix('x') {
            number.parse::<f32>().unwrap_or(1.0) * 1280.0
        } else {
            1280.0
        };
        let score = (width - 1280.0).abs();
        if best
            .as_ref()
            .is_none_or(|(best_score, _)| score < *best_score)
        {
            best = Some((score, url));
        }
    }
    best.map(|(_, url)| url)
}

fn decode_image_bytes(bytes: &[u8]) -> Option<Bitmap> {
    mithu_image::decode(bytes).or_else(|| mithu_image::decode_svg(bytes))
}

fn rasterize_svg_url(url: &str) -> Option<Bitmap> {
    let rest = url.trim().strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    if !meta.to_ascii_lowercase().contains("svg") {
        return None;
    }
    let text = percent_decode(data);
    mithu_image::decode_svg(text.as_bytes())
}

fn percent_decode(input: &str) -> String {
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
    String::from_utf8_lossy(&out).into_owned()
}

fn fetch_bytes(base: &Url, href: &str) -> Option<Vec<u8>> {
    let url = base.resolve(href).ok()?;
    if base.scheme == "https" && url.scheme == "http" {
        return None;
    }
    match url.scheme.as_str() {
        "http" | "https" => {
            let response = if base.host.is_empty() {
                get(&url).ok()?
            } else {
                mithu_net::get_from(&url, &base.host).ok()?
            };
            if response.status >= 400 || response.body.len() > 4_000_000 {
                return None;
            }
            Some(response.body)
        }
        "file" => {
            let bytes = std::fs::read(&url.path).ok()?;
            (bytes.len() <= 4_000_000).then_some(bytes)
        }
        _ => None,
    }
}

fn fetch_text(base: &Url, href: &str) -> Option<String> {
    if let Some(payload) = href.strip_prefix("data:") {
        return decode_data_text(payload);
    }
    if let Some(url) = base.resolve(href).ok().filter(|url| url.scheme == "data") {
        return decode_data_text(&url.path);
    }
    let url = base.resolve(href).ok()?;
    if base.scheme == "https" && url.scheme == "http" {
        return None;
    }
    match url.scheme.as_str() {
        "http" | "https" => {
            let response = if base.host.is_empty() {
                get(&url).ok()?
            } else {
                mithu_net::get_from(&url, &base.host).ok()?
            };
            if response.status >= 400 {
                return None;
            }
            Some(response.text())
        }
        "file" => std::fs::read_to_string(url.path).ok(),
        _ => None,
    }
}

fn origin_key(url: &Url) -> String {
    let port = url.port.unwrap_or(0);
    format!("{}://{}:{port}", url.scheme, url.host)
}

fn decode_data_text(payload: &str) -> Option<String> {
    let (meta, data) = payload.split_once(',')?;
    let bytes = if meta.to_ascii_lowercase().contains(";base64") {
        decode_base64(data.trim())?
    } else {
        percent_bytes(data)
    };
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn percent_bytes(input: &str) -> Vec<u8> {
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
        if bytes[index] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[index]);
        }
        index += 1;
    }
    out
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'=' {
            break;
        }
        let a = value(bytes[index])?;
        let b = value(bytes[index + 1])?;
        out.push((a << 2) | (b >> 4));
        if index + 2 >= bytes.len() || bytes[index + 2] == b'=' {
            break;
        }
        let c = value(bytes[index + 2])?;
        out.push((b << 4) | (c >> 2));
        if index + 3 >= bytes.len() || bytes[index + 3] == b'=' {
            break;
        }
        let d = value(bytes[index + 3])?;
        out.push((c << 6) | d);
        index += 4;
    }
    Some(out)
}

fn bytes_to_html(url: &Url, content_type: Option<&str>, body: &[u8]) -> String {
    let mime = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let text = String::from_utf8_lossy(body);
    let trimmed = text.trim_start();
    if mime.contains("html") || mime.contains("xml") || trimmed.starts_with('<') {
        return text.into_owned();
    }
    if mime.starts_with("text/") || mime.is_empty() {
        return format!("<pre>{}</pre>", escape(&text));
    }
    format!(
        "<h1>Unsupported file</h1><p>{}</p><p>{}</p>",
        escape(&url.to_display()),
        escape(&mime)
    )
}

fn interpret(input: &str) -> Url {
    let text = input.trim();
    if text.is_empty() {
        return Url::parse("about:mithu").expect("about");
    }
    match Url::parse(text) {
        Ok(url) => url,
        Err(UrlError::MissingScheme) if looks_like_host(text) => {
            let scheme = if text.starts_with("localhost") || text.starts_with("127.") {
                "http"
            } else {
                "https"
            };
            Url::parse(&format!("{scheme}://{text}")).unwrap_or_else(|_| search_url(text))
        }
        Err(_) => search_url(text),
    }
}

fn looks_like_host(text: &str) -> bool {
    !text.contains([' ', '\t'])
        && text.contains('.')
        && !text.starts_with('.')
        && text.bytes().all(|byte| byte.is_ascii())
}

fn search_url(text: &str) -> Url {
    let mut query = String::new();
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                query.push(*byte as char)
            }
            b' ' => query.push('+'),
            _ => query.push_str(&format!("%{byte:02X}")),
        }
    }
    Url::parse(&format!("https://duckduckgo.com/?q={query}")).expect("search url")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn error_page(url: &Url, message: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><title>Load failed</title></head><body><h1>This page did not load</h1><p><b>{}</b></p><p>{}</p><p><a href=\"about:mithu\">Back to Mithu</a></p></body></html>",
        escape(&url.to_display()),
        escape(message)
    )
}

fn download_name(url: &Url, mime: &str) -> Option<String> {
    let mime = mime.split(';').next().unwrap_or("").trim();
    let download = mime == "application/pdf"
        || mime == "application/zip"
        || mime == "application/octet-stream"
        || mime == "application/gzip";
    if !download {
        return None;
    }
    let name = url
        .path
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("download");
    Some(name.to_string())
}

fn settings_page() -> String {
    r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Settings</title>
<style>
  body { margin: 0; background: #f2f2f2; color: #1f1f1f; font-family: sans-serif; }
  main { max-width: 720px; margin: 24px auto; background: #fff; border-radius: 16px; padding: 8px 24px 24px; }
  h1 { font-size: 28px; }
  section { border-top: 1px solid #e3e3e3; padding: 16px 0; }
  h2 { font-size: 16px; margin: 0 0 8px; }
  p { margin: 0; color: #5e5e5e; }
  a { color: #0b57d0; text-decoration: none; margin-right: 16px; }
</style>
</head>
<body>
<main>
  <h1>Settings</h1>
  <section>
    <h2>Appearance</h2>
    <p>Light and dark follow the app menu. The frame uses the Chrome Refresh header color.</p>
  </section>
  <section>
    <h2>Search</h2>
    <p>Words typed in the address bar open DuckDuckGo. Hosts open directly.</p>
  </section>
  <section>
    <h2>Privacy</h2>
    <p>Cookies and the HTTP cache stay in this Mithu profile.</p>
  </section>
  <section>
    <h2>Shortcuts</h2>
    <p><a href="about:history">History</a> · <a href="about:downloads">Downloads</a> · <a href="about:mithu">New tab</a></p>
  </section>
</main>
</body>
</html>"#
        .to_string()
}

fn history_page(visits: &[(String, String)]) -> String {
    let mut items = String::new();
    for (title, url) in visits.iter().rev().take(40) {
        items.push_str(&format!(
            "<li><a href=\"{}\">{}</a><div class=\"url\">{}</div></li>",
            escape(url),
            escape(title),
            escape(url)
        ));
    }
    if items.is_empty() {
        items.push_str("<li>No pages yet.</li>");
    }
    format!(
        "<!DOCTYPE html><html><head><title>History</title><style>body{{font-family:sans-serif;margin:24px;color:#1f1f1f;background:#fff}}a{{color:#0b57d0;text-decoration:none}}.url{{color:#5e5e5e;font-size:13px}}</style></head><body><h1>History</h1><ul>{items}</ul></body></html>"
    )
}

fn downloads_page(downloads: &[(String, String)]) -> String {
    let mut items = String::new();
    for (name, url) in downloads.iter().rev() {
        items.push_str(&format!(
            "<li><b>{}</b><div class=\"url\">{}</div></li>",
            escape(name),
            escape(url)
        ));
    }
    if items.is_empty() {
        items.push_str("<li>No downloads yet.</li>");
    }
    format!(
        "<!DOCTYPE html><html><head><title>Downloads</title><style>body{{font-family:sans-serif;margin:24px;color:#1f1f1f;background:#fff}}.url{{color:#5e5e5e;font-size:13px}}</style></head><body><h1>Downloads</h1><ul>{items}</ul></body></html>"
    )
}

fn home_page() -> String {
    r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>New Tab</title>
<style>
  body { margin: 0; background: #f2f2f2; color: #1f1f1f; text-align: center; }
  .wrap { padding-top: 88px; }
  h1 { color: #1f1f1f; font-size: 44px; margin-bottom: 8px; letter-spacing: -1px; }
  .sub { color: #5e5e5e; font-size: 16px; }
  .search { width: 560px; margin-left: auto; margin-right: auto; background: #ffffff; border: 1px solid #c7c7c7; border-radius: 24px; padding: 14px 20px; color: #5e5e5e; margin-top: 28px; text-align: left; }
  .row { display: flex; justify-content: center; gap: 16px; margin-top: 36px; }
  .tile { width: 112px; background: #ffffff; border: 1px solid #e3e3e3; border-radius: 16px; padding: 16px; }
  a { color: #0b57d0; text-decoration: none; }
</style>
</head>
<body>
  <div class="wrap">
    <h1>Mithu</h1>
    <p class="sub">Search or type a URL in the address bar.</p>
    <div class="search">Search the web</div>
    <div class="row">
      <div class="tile"><a href="https://example.com/">Example</a></div>
      <div class="tile"><a href="https://info.cern.ch/">CERN</a></div>
    </div>
  </div>
</body>
</html>"#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srcset_picks_the_candidate_nearest_the_layout_width() {
        let picked = pick_srcset("small.jpg 400w, right.jpg 1200w, huge.jpg 2400w").unwrap();
        assert_eq!(picked, "right.jpg");
    }

    #[test]
    fn home_frame_has_chrome_and_page() {
        let mut browser = Browser::new();
        let frame = browser.render(800.0, 600.0);
        assert_eq!(frame.len(), 800 * 600);
        let strip = frame[4 * 800 + 4];
        assert_eq!(strip, TAB_STRIP.to_u32());
        let toolbar_row = (TAB_H as usize + 8) * 800 + 10;
        assert_eq!(frame[toolbar_row], TOOLBAR.to_u32());
        let page = frame[400 * 800 + 20];
        assert_ne!(page, TOOLBAR.to_u32());
    }

    #[test]
    fn clicking_a_link_navigates() {
        let mut browser = Browser::new();
        let test_url = Url::parse("about:test").unwrap();
        browser.page_mut().history = vec![test_url.clone()];
        browser.page_mut().index = 0;
        browser.install(Fetched {
            url: Url::parse("about:test").unwrap(),
            html: "<a href=\"about:blank\">Go there now</a>".to_string(),
            status: "test".to_string(),
            download_name: None,
        });
        browser.resize(800.0, 600.0);
        let link = browser
            .page()
            .layout
            .links
            .iter()
            .find(|link| link.href.contains("about:blank"))
            .expect("link")
            .rect;
        browser.pointer_down(link.x + 2.0, link.y + 2.0 + CONTENT_TOP);
        assert_eq!(browser.location(), "about:blank");
        assert!(browser.can_back());
        browser.back();
        assert_eq!(browser.location(), "about:test");
    }

    #[test]
    fn a_data_image_paints_its_pixel() {
        let mut browser = Browser::new();
        browser.install(Fetched {
            url: Url::parse("about:test").unwrap(),
            html: "<img id=\"pic\" alt=\"red\" src=\"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==\">".to_string(),
            status: "test".to_string(),
            download_name: None,
        });
        browser.resize(800.0, 600.0);
        let pic = browser.page().document.get_element_by_id("pic").unwrap();
        let laid = find_box(&browser.page().layout.root, pic).expect("image box");
        assert!(laid.image.is_some());
        let x = (laid.rect.x + laid.rect.width / 2.0).floor() as usize;
        let y = (laid.rect.y + laid.rect.height / 2.0 + CONTENT_TOP).floor() as usize;
        let frame = browser.render(800.0, 600.0);
        assert_eq!(
            frame[y * 800 + x],
            0x00FF_0000,
            "pixel {x},{y} is {:06x}",
            frame[y * 800 + x]
        );
    }

    #[test]
    fn a_click_runs_the_listener_and_a_form_navigates() {
        let mut browser = Browser::new();
        browser.install(Fetched {
            url: Url::parse("about:test").unwrap(),
            html: r#"<button id="go">Go</button><p id="out">old</p>
                <a id="stay" href="about:blank">Stay</a>
                <form action="about:blank"><input name="q" value="mithu"><button id="send" type="submit">Send</button></form>
                <script>
                  document.getElementById("go").addEventListener("click", function () {
                    document.getElementById("out").textContent = "clicked";
                  });
                  document.getElementById("stay").addEventListener("click", function (event) {
                    event.preventDefault();
                  });
                </script>"#
                .to_string(),
            status: "test".to_string(),
            download_name: None,
        });
        click_id(&mut browser, "go");
        let out = browser.page().document.get_element_by_id("out").unwrap();
        assert_eq!(browser.page().document.text_content(out), "clicked");
        click_id(&mut browser, "stay");
        assert_eq!(browser.location(), "about:test");
        click_id(&mut browser, "send");
        assert_eq!(browser.page().current.query.as_deref(), Some("q=mithu"));
        assert_eq!(browser.location(), "about:blank");
    }

    #[test]
    fn tabs_keep_separate_pages() {
        let mut browser = Browser::new();
        browser.install(Fetched {
            url: Url::parse("about:test").unwrap(),
            html: "<title>One</title><p>one</p>".to_string(),
            status: "test".to_string(),
            download_name: None,
        });
        browser.new_tab();
        assert_eq!(browser.tab_count(), 2);
        assert_eq!(browser.active_tab(), 1);
        assert_eq!(browser.location(), "about:mithu");
        browser.select_tab(0);
        assert_eq!(browser.location(), "about:test");
        assert_eq!(browser.title(), "One");
        let (tabs, plus) = tab_rects(browser.viewport_w, browser.tab_count());
        browser.pointer_down(plus.x + 4.0, plus.y + 4.0);
        assert_eq!(browser.tab_count(), 3);
        browser.pointer_down(tabs[0].x + 4.0, tabs[0].y + 4.0);
        assert_eq!(browser.active_tab(), 0);
        browser.close_tab();
        assert_eq!(browser.tab_count(), 2);
        assert_eq!(browser.location(), "about:mithu");
    }

    #[test]
    fn find_wraps_matches_and_scrolls_to_the_active_one() {
        let mut browser = Browser::new();
        browser.resize(800.0, 600.0);
        browser.install(Fetched {
            url: Url::parse("about:test").unwrap(),
            html: "<style>p{margin:0}</style><p>alpha beta alpha</p><div style=\"height:1800px\"></div><p>alpha tail</p>".to_string(),
            status: "test".to_string(),
            download_name: None,
        });
        browser.open_find();
        for ch in "Alpha".chars() {
            browser.find_insert(ch);
        }
        assert_eq!(browser.find_count(), 3);
        assert_eq!(browser.find_active(), Some(0));
        assert!(browser.page().scroll < 20.0);
        browser.find_next();
        browser.find_next();
        assert_eq!(browser.find_active(), Some(2));
        assert!(
            browser.page().scroll > 400.0,
            "scroll {}",
            browser.page().scroll
        );
        browser.find_next();
        assert_eq!(browser.find_active(), Some(0));
        let frame = browser.render(800.0, 600.0);
        let bar_y = (600.0 - STATUS_H - FIND_H / 2.0) as usize;
        assert_eq!(frame[bar_y * 800 + 20], FIND_BAR.to_u32());
        browser.close_find();
        assert!(!browser.find_is_open());
        assert_eq!(browser.find_count(), 0);
    }

    #[test]
    fn zoom_scales_the_page_and_reflows() {
        let mut browser = Browser::new();
        browser.resize(800.0, 600.0);
        browser.install(Fetched {
            url: Url::parse("about:test").unwrap(),
            html: "<div style=\"width:100px;height:40px;background:#ff0000;margin:0\"></div>"
                .to_string(),
            status: "test".to_string(),
            download_name: None,
        });
        let y = (CONTENT_TOP + 20.0) as usize;
        let frame = browser.render(800.0, 600.0);
        assert_eq!(frame[y * 800 + 20], 0x00FF_0000);
        assert_ne!(frame[y * 800 + 150], 0x00FF_0000);
        while browser.zoom() < 1.99 {
            browser.zoom_in();
        }
        assert!((browser.zoom() - 2.0).abs() < 0.01);
        let frame = browser.render(800.0, 600.0);
        assert_eq!(
            frame[y * 800 + 40],
            0x00FF_0000,
            "scaled pixel {:06x}",
            frame[y * 800 + 40]
        );
        assert_eq!(frame[y * 800 + 150], 0x00FF_0000);
        browser.zoom_reset();
        assert!((browser.zoom() - 1.0).abs() < 0.01);
        let frame = browser.render(800.0, 600.0);
        assert_ne!(frame[y * 800 + 150], 0x00FF_0000);
    }

    #[test]
    fn bookmarks_save_and_reopen_a_page() {
        let mut browser = Browser::new();
        browser.resize(800.0, 600.0);
        assert_eq!(browser.location(), "about:mithu");
        assert!(!browser.is_bookmarked());
        browser.toggle_bookmark();
        assert!(browser.is_bookmarked());
        browser.toggle_bookmark();
        assert_eq!(browser.bookmark_count(), 0);
        browser.toggle_bookmark();
        browser.navigate("about:blank");
        assert!(!browser.is_bookmarked());
        browser.toggle_bookmark_list();
        let panel = bookmark_panel(800.0, browser.bookmark_count());
        browser.pointer_down(panel.x + 20.0, panel.y + 10.0);
        assert_eq!(browser.location(), "about:mithu");
        assert_eq!(browser.title(), "New Tab");
        assert!(!browser.bookmarks_open());
        let (star, _) = bookmark_buttons(800.0);
        let frame = browser.render(800.0, 600.0);
        let sx = star.x as usize + 4;
        let sy = star.y as usize + 4;
        assert_eq!(frame[sy * 800 + sx], BOOKMARKED.to_u32());
        browser.navigate("about:blank");
        let slots = bookmark_slots(800.0, browser.bookmark_count());
        browser.pointer_down(slots[0].x + 12.0, slots[0].y + 8.0);
        assert_eq!(browser.location(), "about:mithu");
    }

    #[test]
    fn address_suggestions_open_a_visited_page() {
        let mut browser = Browser::new();
        browser.resize(800.0, 600.0);
        browser.navigate("about:blank");
        browser.focus_omnibox();
        for ch in "mithu".chars() {
            browser.insert_char(ch);
        }
        let items = browser.suggestions();
        let index = items
            .iter()
            .position(|item| item.url.to_display() == "about:mithu")
            .expect("suggestion");
        assert!(index > 0);
        for _ in 0..=index {
            browser.move_suggestion(1);
        }
        browser.submit_omnibox();
        assert_eq!(browser.location(), "about:mithu");
        assert!(!browser.omnibox_focused);

        browser.focus_omnibox();
        for ch in "mithu".chars() {
            browser.insert_char(ch);
        }
        let panel = suggestion_panel(800.0, browser.suggestions().len());
        let row_y = panel.y + index as f32 * 32.0 + 10.0;
        browser.pointer_down(panel.x + 12.0, row_y);
        assert_eq!(browser.location(), "about:mithu");
    }

    #[test]
    fn the_context_menu_opens_a_link_in_a_new_tab() {
        let mut browser = Browser::new();
        browser.resize(800.0, 600.0);
        browser.install(Fetched {
            url: Url::parse("about:test").unwrap(),
            html: "<a href=\"about:blank\">Elsewhere</a>".to_string(),
            status: "test".to_string(),
            download_name: None,
        });
        let link = browser
            .page()
            .layout
            .links
            .iter()
            .find(|link| link.href.contains("about:blank"))
            .expect("link")
            .rect;
        browser.context_menu(link.x + 2.0, link.y + 2.0 + CONTENT_TOP);
        let menu = browser.menu.as_ref().expect("menu");
        let new_tab = menu
            .items
            .iter()
            .position(|(label, _)| label == "Open in new tab")
            .expect("item");
        let bounds = menu_bounds(menu, 800.0, 600.0);
        browser.pointer_down(bounds.x + 12.0, bounds.y + new_tab as f32 * 32.0 + 10.0);
        assert_eq!(browser.tab_count(), 2);
        assert_eq!(browser.location(), "about:blank");
        assert!(!browser.menu_open());
        browser.select_tab(0);
        assert_eq!(browser.location(), "about:test");
        browser.context_menu(20.0, CONTENT_TOP + 20.0);
        let menu = browser.menu.as_ref().expect("menu");
        let reload = menu
            .items
            .iter()
            .position(|(label, _)| label == "Reload")
            .expect("reload");
        let bounds = menu_bounds(menu, 800.0, 600.0);
        browser.pointer_down(bounds.x + 12.0, bounds.y + reload as f32 * 32.0 + 10.0);
        assert_eq!(browser.location(), "about:test");
    }

    #[test]
    fn dragging_a_tab_reorders_it_and_history_lists_the_visit() {
        let mut browser = Browser::new();
        browser.navigate("about:blank");
        browser.new_tab();
        assert_eq!(browser.location(), "about:mithu");
        let (tabs, _) = tab_rects(browser.viewport_w, browser.tab_count());
        browser.pointer_down(tabs[1].x + 4.0, tabs[1].y + 4.0);
        browser.pointer_move(tabs[0].x + 2.0, tabs[0].y + 4.0);
        browser.pointer_up();
        assert_eq!(browser.active_tab(), 0);
        assert_eq!(browser.location(), "about:mithu");
        browser.select_tab(1);
        assert_eq!(browser.location(), "about:blank");
        browser.navigate("about:history");
        assert!(
            browser
                .page()
                .document
                .text_content(browser.page().document.root)
                .contains("about:blank"),
            "history should list the blank page"
        );
    }

    #[test]
    fn toolbar_icon_is_vector_ink_and_the_close_button_removes_a_tab() {
        let mut browser = Browser::new();
        browser.navigate("about:blank");
        let frame = browser.render(800.0, 600.0);
        let (back, _, _, _) = chrome_rects(800.0);
        let x = (back.x + back.width / 2.0) as usize;
        let y = (back.y + back.height / 2.0) as usize;
        assert_ne!(
            frame[y * 800 + x],
            TOOLBAR.to_u32(),
            "the back arrow should paint inside its button"
        );
        browser.new_tab();
        let strip = browser.tab_strip();
        let close = strip.closes[1];
        browser.pointer_move(close.x + 4.0, close.y + 4.0);
        browser.pointer_down(close.x + 4.0, close.y + 4.0);
        assert_eq!(browser.tab_count(), 1);
        browser.reopen_closed_tab();
        assert_eq!(browser.tab_count(), 2);
    }

    #[test]
    fn dark_theme_and_settings_page_follow_the_menu() {
        let mut browser = Browser::new();
        browser.toggle_dark();
        let frame = browser.render(800.0, 600.0);
        assert_ne!(frame[4 * 800 + 4], TAB_STRIP.to_u32());
        browser.navigate("about:settings");
        assert_eq!(browser.title(), "Settings");
        assert!(
            browser
                .page()
                .document
                .text_content(browser.page().document.root)
                .contains("Appearance")
        );
    }

    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn fetch_then_updates_html_and_the_image_before_paint() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..8 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buf = [0u8; 2048];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let request = String::from_utf8_lossy(&buf);
                let (kind, body) = if request.contains("GET /words") {
                    ("text/plain", b"Filled".to_vec())
                } else if request.contains("GET /pic.png") {
                    ("image/png", TINY_PNG.to_vec())
                } else {
                    let html = format!(
                        r#"<!doctype html><div id="host"></div><img id="pic" alt="pic">
                        <script type="module">document.body.dataset.mod="1"</script>
                        <script>
                          requestAnimationFrame(() => document.body.dataset.raf = "1");
                          requestIdleCallback(() => document.body.dataset.idle = "1");
                          new IntersectionObserver(entries => {{
                            if (entries[0] && entries[0].isIntersecting) document.body.dataset.io = "1";
                          }}).observe(document.getElementById("host"));
                          fetch("/words").then(r => r.text()).then(text => {{
                            document.getElementById("host").innerHTML = "<b id='filled'>" + text + "</b>";
                            document.getElementById("pic").src = "/pic.png";
                          }});
                        </script>"#
                    );
                    ("text/html", html.into_bytes())
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = std::io::Write::write_all(&mut stream, head.as_bytes());
                let _ = std::io::Write::write_all(&mut stream, &body);
            }
        });
        let mut browser = Browser::new();
        browser.navigate(&format!("http://127.0.0.1:{port}/"));
        browser.advance_timers(16);
        let page = browser.page();
        let text = page.document.text_content(page.document.root);
        assert!(text.contains("Filled"), "{text}");
        let pic = page.document.get_element_by_id("pic").unwrap();
        assert_eq!(page.document.attr(pic, "src"), Some("/pic.png"));
        assert!(page.images.contains_key(&pic), "image should be decoded");
        let body = page
            .document
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, _)| {
                (page.document.element_name(mithu_dom::NodeId(index)) == Some("body"))
                    .then_some(mithu_dom::NodeId(index))
            })
            .unwrap();
        assert_eq!(page.document.attr(body, "data-io"), Some("1"));
        assert_eq!(page.document.attr(body, "data-raf"), Some("1"));
        assert_eq!(page.document.attr(body, "data-idle"), Some("1"));
        assert_eq!(page.document.attr(body, "data-mod"), Some("1"));
    }

    fn click_id(browser: &mut Browser, id: &str) {
        let (x, y) = {
            let page = browser.page();
            let node = page.document.get_element_by_id(id).unwrap();
            if let Some(laid) = find_box(&page.layout.root, node) {
                (
                    laid.rect.x + laid.rect.width / 2.0,
                    laid.rect.y + laid.rect.height / 2.0,
                )
            } else if let Some(href) = page.document.attr(node, "href") {
                let link = page
                    .layout
                    .links
                    .iter()
                    .find(|link| link.href == href)
                    .unwrap_or_else(|| panic!("missing link {id}"));
                (link.rect.x + 2.0, link.rect.y + 2.0)
            } else {
                panic!("missing box {id}")
            }
        };
        browser.pointer_down(x, y + CONTENT_TOP);
    }

    fn find_box(box_: &mithu_layout::LayoutBox, id: NodeId) -> Option<&mithu_layout::LayoutBox> {
        if box_.node == id {
            return Some(box_);
        }
        box_.children.iter().find_map(|child| find_box(child, id))
    }
}
