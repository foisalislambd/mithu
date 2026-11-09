//! Browser shell: navigation, history, chrome, and a headless frame renderer.
//!
//! The window crate only forwards input. Loading, layout, and painting live here
//! so the engine can be tested without a display server.

use std::collections::HashMap;
use std::sync::Arc;

use mithu_base::{Color, Rect};
use mithu_dom::{Document, NodeId};
use mithu_html::parse_html;
use mithu_image::Bitmap;
use mithu_js::{Session, Submission};
use mithu_layout::{hit_test, layout_tree, LayoutTree};
use mithu_net::{get, post};
use mithu_paint::{paint_layout, Canvas};
use mithu_style::{style_document, ComputedStyle};
use mithu_text::{FontBook, MeasureText};
use mithu_url::{Url, UrlError};

pub const TAB_H: f32 = 40.0;
pub const TOOLBAR_H: f32 = 56.0;
pub const CONTENT_TOP: f32 = TAB_H + TOOLBAR_H;
pub const FIND_H: f32 = 36.0;
pub const STATUS_H: f32 = 26.0;

const TOOLBAR: Color = Color::rgb(22, 52, 47);
const BUTTON: Color = Color::rgb(32, 74, 66);
const BUTTON_TEXT: Color = Color::rgb(244, 247, 244);
const BUTTON_DISABLED: Color = Color::rgb(132, 156, 150);
const ADDRESS: Color = Color::rgb(255, 253, 248);
const ADDRESS_BORDER: Color = Color::rgb(196, 188, 170);
const ADDRESS_FOCUS: Color = Color::rgb(244, 208, 122);
const STATUS_BG: Color = Color::rgb(236, 232, 223);
const STATUS_TEXT: Color = Color::rgb(62, 56, 48);
const SELECTION: Color = Color::rgb(196, 220, 246);
const FIND_HIT: Color = Color::rgb(255, 214, 64);
const FIND_ACTIVE: Color = Color::rgb(255, 140, 40);
const FIND_BAR: Color = Color::rgb(255, 248, 220);
const STAR: Color = Color::rgb(244, 196, 64);
const ZOOMS: [f32; 17] = [0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];

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
            layout: layout_tree(&Document::new(), &[], width, &mithu_text::NaiveMetrics, &HashMap::new()),
            images: HashMap::new(),
            session: None,
            focused: None,
            find_query: String::new(),
            find_open: false,
            find_at: 0,
            zoom: 1.0,
        }
    }
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
}

struct Fetched {
    url: Url,
    html: String,
    status: String,
}

impl Browser {
    fn page(&self) -> &Page {
        self.tabs.page()
    }

    fn page_mut(&mut self) -> &mut Page {
        self.tabs.page_mut()
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
        };
        browser.navigate("about:mithu");
        browser
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

    pub fn toggle_bookmark(&mut self) {
        let url = self.page().current.clone();
        let key = url.to_display();
        if let Some(index) = self.bookmarks.iter().position(|item| item.url.to_display() == key) {
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
        self.bookmarks.iter().any(|item| item.url.to_display() == key)
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
            ZOOMS.into_iter().find(|level| *level > current + 0.001).unwrap_or(ZOOMS[ZOOMS.len() - 1])
        } else {
            ZOOMS.into_iter().rev().find(|level| *level < current - 0.001).unwrap_or(ZOOMS[0])
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
        self.omnibox_focused = false;
        self.find_focused = false;
        if self.tabs.pages.len() == 1 {
            self.navigate("about:mithu");
            return;
        }
        let index = self.tabs.active;
        self.tabs.pages.remove(index);
        if self.tabs.active >= self.tabs.pages.len() {
            self.tabs.active = self.tabs.pages.len() - 1;
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
        self.load_url(url);
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
        self.load_url(url);
    }

    pub fn reload(&mut self) {
        let url = self.page().current.clone();
        self.load_url(url);
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
        if y < TAB_H {
            let (tabs, plus) = tab_rects(self.viewport_w, self.tab_count());
            if plus.contains(x, y) {
                self.new_tab();
            } else if let Some(index) = tabs.iter().position(|rect| rect.contains(x, y)) {
                self.select_tab(index);
            }
            return;
        }
        if y < CONTENT_TOP {
            let (back, forward, reload, address) = chrome_rects(self.viewport_w);
            if back.contains(x, y) && self.can_back() {
                self.back();
            } else if forward.contains(x, y) && self.can_forward() {
                self.forward();
            } else if reload.contains(x, y) {
                self.reload();
            } else if address.contains(x, y) {
                self.focus_omnibox();
            } else {
                let (star, list) = bookmark_buttons(self.viewport_w);
                if star.contains(x, y) {
                    self.toggle_bookmark();
                } else if list.contains(x, y) {
                    self.toggle_bookmark_list();
                }
            }
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
        }
    }

    fn open_in_new_tab(&mut self, href: String) {
        let url = {
            let page = self.page();
            page.base.resolve(&href).unwrap_or_else(|_| interpret(&href))
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
        let submission = self.page_mut().session.as_mut().and_then(|session| session.submission(id));
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
        items.push(Suggestion { label: typed_label, url: typed });
        let needle = query.to_ascii_lowercase();
        for bookmark in &self.bookmarks {
            if items.len() >= 8 {
                break;
            }
            let hay = format!("{} {}", bookmark.title, bookmark.url.to_display()).to_ascii_lowercase();
            if hay.contains(&needle) && items.iter().all(|item| item.url.to_display() != bookmark.url.to_display()) {
                items.push(Suggestion {
                    label: bookmark.title.clone(),
                    url: bookmark.url.clone(),
                });
            }
        }
        for url in self.page().history.iter().rev() {
            if items.len() >= 8 {
                break;
            }
            let display = url.to_display();
            if display.to_ascii_lowercase().contains(&needle) && items.iter().all(|item| item.url.to_display() != display) {
                items.push(Suggestion { label: display, url: url.clone() });
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
            self.load_url(url);
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
        self.load_url(url);
    }

    fn load_url(&mut self, url: Url) {
        self.page_mut().status = format!("Loading {}", url.to_display());
        match retrieve(&url) {
            Ok(fetched) => self.install(fetched),
            Err(message) => self.install(Fetched {
                html: error_page(&url, &message),
                status: message,
                url,
            }),
        }
    }

    fn install(&mut self, fetched: Fetched) {
        let find_query = self.page().find_query.clone();
        let find_open = self.page().find_open;
        let zoom = self.page().zoom;
        let layout_width = self.layout_width();
        let document = parse_html(&fetched.html);
        let base = match &document.base_href {
            Some(href) => fetched.url.resolve(href).unwrap_or_else(|_| fetched.url.clone()),
            None => fetched.url.clone(),
        };
        let mut session = Session::open(document, |src| fetch_text(&base, src));
        let logs = session.logs();
        session.edit(|doc| doc.refresh_metadata());
        let document = session.document();
        let mut author = String::new();
        for sheet in &document.stylesheets {
            author.push_str(sheet);
            author.push('\n');
        }
        for href in document.stylesheet_links.iter().take(6) {
            if let Some(css) = fetch_text(&base, href)
                && css.len() <= 500_000
            {
                author.push_str(&css);
                author.push('\n');
            }
        }
        let styles = style_document(&document, &author);
        let images = load_images(&document, &base);
        let layout = layout_tree(&document, &styles, layout_width, &self.fonts, &images);
        self.omnibox_focused = false;
        let page = self.page_mut();
        page.title = if document.title.is_empty() {
            fetched.url.to_display()
        } else {
            document.title.clone()
        };
        page.status = if logs.is_empty() {
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
        page.find_query = find_query;
        page.find_open = find_open;
        page.find_at = 0;
        page.zoom = zoom;
        let reopen = page.find_open && !page.find_query.is_empty();
        let _ = page;
        if reopen {
            self.reveal_find();
        }
    }

    fn layout_width(&self) -> f32 {
        (self.viewport_w / self.page().zoom.max(0.01)).max(1.0)
    }

    fn relayout(&mut self) {
        let width = self.layout_width();
        let layout = {
            let page = self.tabs.page();
            layout_tree(&page.document, &page.styles, width, &self.fonts, &page.images)
        };
        self.page_mut().layout = layout;
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
            if allow_default
                && let Some(field) = nearest(&document, node, is_text_field)
            {
                focus_field = Some(field);
            } else if allow_default
                && let Some(control) = nearest(&document, node, is_checkbox)
            {
                session.edit(|doc| toggle_checked(doc, control));
                session.dispatch(control, "change");
            } else if allow_default
                && let Some(control) = nearest(&document, node, is_submit)
            {
                submission = session.submission(control);
            }
        }
        self.pull_page();
        self.page_mut().focused = focus_field;
        if let Some(submission) = submission {
            self.send_form(submission);
            return;
        }
        if allow_default && focus_field.is_none()
            && let Some(href) = self.hit_link(x, y)
        {
            self.follow(href);
        }
    }

    fn pull_page(&mut self) {
        let base = self.page().base.clone();
        let (document, logs) = {
            let Some(session) = self.page_mut().session.as_mut() else {
                return;
            };
            session.edit(|doc| doc.refresh_metadata());
            (session.document(), session.logs())
        };
        let author = author_css(&document, &base);
        let styles = style_document(&document, &author);
        let images = load_images(&document, &base);
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
        }
        self.relayout();
    }

    fn send_form(&mut self, submission: Submission) {
        let mut url = if submission.action.is_empty() {
            self.page().current.clone()
        } else {
            self.page().base
                .resolve(&submission.action)
                .unwrap_or_else(|_| interpret(&submission.action))
        };
        if submission.method == "post" && url.is_network() {
            match post(&url, "application/x-www-form-urlencoded", submission.query.as_bytes()) {
                Ok(response) => {
                    let status = format!("{} {}", response.status, response.url.to_display());
                    let html = bytes_to_html(&response.url, response.content_type(), &response.body);
                    self.install(Fetched {
                        url: response.url,
                        html,
                        status,
                    });
                }
                Err(error) => self.install(Fetched {
                    html: error_page(&url, &error.to_string()),
                    status: error.to_string(),
                    url,
                }),
            }
            return;
        }
        if !submission.query.is_empty() {
            url.query = Some(match url.query.as_deref() {
                Some(existing) if !existing.is_empty() => format!("{existing}&{}", submission.query),
                _ => submission.query,
            });
        }
        self.navigate_url(url);
    }

    fn follow(&mut self, href: String) {
        let url = self.page().base.resolve(&href).unwrap_or_else(|_| interpret(&href));
        self.navigate_url(url);
    }

    fn hit_link(&self, x: f32, y: f32) -> Option<String> {
        self.page().layout.links.iter().rev().find(|link| link.rect.contains(x, y)).map(|link| link.href.clone())
    }

    fn can_back(&self) -> bool {
        self.page().index > 0
    }

    fn can_forward(&self) -> bool {
        self.page().index + 1 < self.page().history.len()
    }

    fn device_page_height(&self) -> f32 {
        let find = if self.page().find_open { FIND_H } else { 0.0 };
        (self.viewport_h - CONTENT_TOP - STATUS_H - find).max(1.0)
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
        let clip = Rect::new(0.0, origin_y, width as f32, (height as f32 - origin_y - STATUS_H - find).max(1.0));
        let mut canvas = Canvas {
            pixels,
            width: width as i32,
            height: height as i32,
        };
        self.paint_find_hits(&mut canvas, scroll, clip, origin_y);
        paint_layout(&mut canvas, &self.tabs.page().layout, &self.fonts, clip, scroll, origin_y);
    }

    fn paint_zoomed_page(&self, pixels: &mut [u32], width: u32, height: u32, zoom: f32) {
        let scroll = self.page().scroll;
        let find = if self.page().find_open { FIND_H } else { 0.0 };
        let device_h = (height as f32 - CONTENT_TOP - STATUS_H - find).max(1.0);
        let css_w = ((width as f32) / zoom).round().max(1.0) as i32;
        let css_h = (device_h / zoom).round().max(1.0) as i32;
        let mut page_pixels = vec![self.page().layout.background.to_u32(); (css_w * css_h) as usize];
        let clip = Rect::new(0.0, 0.0, css_w as f32, css_h as f32);
        {
            let mut canvas = Canvas {
                pixels: &mut page_pixels,
                width: css_w,
                height: css_h,
            };
            self.paint_find_hits(&mut canvas, scroll, clip, 0.0);
            paint_layout(&mut canvas, &self.tabs.page().layout, &self.fonts, clip, scroll, 0.0);
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
        let full = Rect::new(0.0, 0.0, width, height);
        canvas.fill(Rect::new(0.0, 0.0, width, CONTENT_TOP), TOOLBAR, full);
        self.paint_tabs(canvas);
        let (back, forward, reload, address) = chrome_rects(width);
        self.paint_button(canvas, back, "<", self.can_back());
        self.paint_button(canvas, forward, ">", self.can_forward());
        self.paint_button(canvas, reload, "R", true);
        let (star, list) = bookmark_buttons(width);
        self.paint_button(canvas, star, "*", self.is_bookmarked());
        if self.is_bookmarked() {
            canvas.fill(star, STAR, full);
            canvas.text(
                &self.fonts,
                star.x + 11.0,
                star.y + 21.0,
                "*",
                16.0,
                Color::rgb(48, 32, 8),
                true,
                false,
                false,
                star,
            );
        }
        self.paint_button(canvas, list, "B", self.bookmarks_open);
        let border = if self.omnibox_focused { ADDRESS_FOCUS } else { ADDRESS_BORDER };
        canvas.fill(address, border, full);
        let inset = 2.0;
        canvas.fill(
            Rect::new(address.x + inset, address.y + inset, address.width - inset * 2.0, address.height - inset * 2.0),
            ADDRESS,
            full,
        );
        let text_clip = Rect::new(address.x + 8.0, address.y + 4.0, (address.width - 16.0).max(1.0), address.height - 8.0);
        let size = 16.0;
        let prefix: String = self.page().omnibox.chars().take(self.page().cursor).collect();
        let prefix_w = self.fonts.width(&prefix, size, false, false);
        let full_w = self.fonts.width(&self.page().omnibox, size, false, false);
        let mut shift = 0.0;
        if full_w > text_clip.width {
            shift = (prefix_w - text_clip.width + 12.0).max(0.0);
        }
        let text_x = text_clip.x - shift;
        let baseline = address.y + 22.0;
        if self.omnibox_focused && self.page().select_all && !self.page().omnibox.is_empty() {
            canvas.fill(Rect::new(text_x - 2.0, address.y + 6.0, full_w + 4.0, 22.0), SELECTION, text_clip);
        }
        canvas.text(
            &self.fonts,
            text_x,
            baseline,
            &self.page().omnibox,
            size,
            Color::rgb(28, 26, 22),
            false,
            false,
            false,
            text_clip,
        );
        if self.omnibox_focused && !self.page().select_all {
            canvas.fill(Rect::new(text_x + prefix_w, address.y + 8.0, 1.5, 18.0), Color::rgb(22, 52, 47), text_clip);
        }
        canvas.fill(Rect::new(0.0, height - STATUS_H, width, STATUS_H), STATUS_BG, full);
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
            STATUS_TEXT,
            false,
            false,
            false,
            Rect::new(8.0, height - STATUS_H, width - 16.0, STATUS_H),
        );
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
    }

    fn paint_menu(&self, canvas: &mut Canvas<'_>, menu: &Menu) {
        let bounds = menu_bounds(menu, canvas.width as f32, canvas.height as f32);
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        canvas.fill(bounds, Color::rgb(40, 36, 32), full);
        for (index, (label, _)) in menu.items.iter().enumerate() {
            let row = Rect::new(bounds.x + 4.0, bounds.y + index as f32 * 32.0 + 2.0, bounds.width - 8.0, 28.0);
            canvas.fill(row, ADDRESS, full);
            canvas.text(
                &self.fonts,
                row.x + 8.0,
                row.y + 19.0,
                label,
                14.0,
                Color::rgb(28, 26, 22),
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
        canvas.fill(panel, Color::rgb(40, 36, 32), full);
        for (index, item) in items.iter().enumerate() {
            let row = Rect::new(panel.x + 4.0, panel.y + index as f32 * 32.0 + 2.0, panel.width - 8.0, 28.0);
            let fill = if self.suggestion == Some(index) { SELECTION } else { ADDRESS };
            canvas.fill(row, fill, full);
            canvas.text(
                &self.fonts,
                row.x + 8.0,
                row.y + 19.0,
                &item.label,
                14.0,
                Color::rgb(28, 26, 22),
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
        canvas.fill(panel, Color::rgb(40, 36, 32), full);
        if self.bookmarks.is_empty() {
            canvas.text(
                &self.fonts,
                panel.x + 12.0,
                panel.y + 21.0,
                "No bookmarks",
                14.0,
                BUTTON_TEXT,
                false,
                false,
                false,
                panel,
            );
            return;
        }
        for (index, bookmark) in self.bookmarks.iter().enumerate().take(8) {
            let row = Rect::new(panel.x + 4.0, panel.y + index as f32 * 32.0 + 2.0, panel.width - 8.0, 28.0);
            canvas.fill(row, ADDRESS, full);
            canvas.text(
                &self.fonts,
                row.x + 8.0,
                row.y + 19.0,
                &bookmark.title,
                14.0,
                Color::rgb(28, 26, 22),
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
            let color = if index == active { FIND_ACTIVE } else { FIND_HIT };
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
        let border = if self.find_focused { ADDRESS_FOCUS } else { ADDRESS_BORDER };
        canvas.fill(bar, border, full);
        canvas.fill(Rect::new(bar.x + 2.0, bar.y + 2.0, bar.width - 4.0, bar.height - 4.0), FIND_BAR, full);
        let count = self.find_hits().len();
        let label = if self.page().find_query.is_empty() {
            "Find".to_string()
        } else if count == 0 {
            format!("{}   0/0", self.page().find_query)
        } else {
            format!("{}   {}/{count}", self.page().find_query, self.page().find_at.min(count - 1) + 1)
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

    fn paint_tabs(&self, canvas: &mut Canvas<'_>) {
        let (rects, plus) = tab_rects(canvas.width as f32, self.tabs.pages.len());
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        for (index, rect) in rects.iter().enumerate() {
            let active = index == self.tabs.active;
            canvas.fill(*rect, if active { ADDRESS } else { BUTTON }, full);
            let title = {
                let page = &self.tabs.pages[index];
                if page.title.is_empty() { page.current.to_display() } else { page.title.clone() }
            };
            canvas.text(&self.fonts, rect.x + 8.0, rect.y + 18.0, &title, 13.0, Color::rgb(28, 26, 22), active, false, false, *rect);
        }
        canvas.fill(plus, BUTTON, full);
        canvas.text(&self.fonts, plus.x + 8.0, plus.y + 18.0, "+", 16.0, BUTTON_TEXT, true, false, false, plus);
    }

    fn paint_button(&self, canvas: &mut Canvas<'_>, rect: Rect, label: &str, enabled: bool) {
        let full = Rect::new(0.0, 0.0, canvas.width as f32, canvas.height as f32);
        canvas.fill(rect, BUTTON, full);
        let size = 16.0;
        let width = self.fonts.width(label, size, true, false);
        let color = if enabled { BUTTON_TEXT } else { BUTTON_DISABLED };
        canvas.text(
            &self.fonts,
            rect.x + (rect.width - width) / 2.0,
            rect.y + 21.0,
            label,
            size,
            color,
            true,
            false,
            false,
            rect,
        );
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
        if index > 0 && (run.baseline - runs[index - 1].baseline).abs() < 0.5 && run.x > runs[index - 1].x + 0.5 {
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
            while cursor < end && slots[cursor].is_some_and(|(next_run, next_char)| next_run == run_index && next_char == char_end)
            {
                char_end += 1;
                cursor += 1;
            }
            let run = &runs[run_index];
            let prefix: String = run.text.chars().take(char_start).collect();
            let slice: String = run.text.chars().skip(char_start).take(char_end - char_start).collect();
            let x = run.x + fonts.width(&prefix, run.size, run.bold, run.mono);
            let width = fonts.width(&slice, run.size, run.bold, run.mono).max(1.0);
            rects.push(Rect::new(x, run.baseline - run.ascent, width, run.height.max(1.0)));
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

fn tab_rects(width: f32, count: usize) -> (Vec<Rect>, Rect) {
    let count = count.max(1) as f32;
    let available = (width - 52.0).max(80.0);
    let tab_w = (available / count - 4.0).clamp(48.0, 180.0);
    let rects = (0..count as usize)
        .map(|index| Rect::new(8.0 + index as f32 * (tab_w + 4.0), 6.0, tab_w, TAB_H - 12.0))
        .collect();
    let plus = Rect::new(8.0 + count * (tab_w + 4.0), 6.0, 28.0, TAB_H - 12.0);
    (rects, plus)
}

struct Suggestion {
    label: String,
    url: Url,
}

#[derive(Clone)]
enum MenuAction {
    Open(String),
    NewTab(String),
    Back,
    Forward,
    Reload,
    Bookmark,
}

#[derive(Clone)]
struct Menu {
    x: f32,
    y: f32,
    items: Vec<(String, MenuAction)>,
}

fn menu_bounds(menu: &Menu, width: f32, height: f32) -> Rect {
    let menu_w = 220.0;
    let menu_h = (menu.items.len().max(1) as f32) * 32.0;
    let x = menu.x.min((width - menu_w - 4.0).max(4.0)).max(4.0);
    let y = menu.y.min((height - menu_h - 4.0).max(4.0)).max(4.0);
    Rect::new(x, y, menu_w, menu_h)
}

fn suggestion_panel(width: f32, count: usize) -> Rect {
    let (_, _, _, address) = chrome_rects(width);
    Rect::new(address.x, address.bottom(), address.width, count.max(1) as f32 * 32.0)
}

fn bookmark_buttons(width: f32) -> (Rect, Rect) {
    let y = TAB_H + 12.0;
    (Rect::new(width - 84.0, y, 32.0, 32.0), Rect::new(width - 44.0, y, 32.0, 32.0))
}

fn bookmark_panel(width: f32, count: usize) -> Rect {
    let rows = count.clamp(1, 8) as f32;
    let panel_w = 280.0_f32.min((width - 16.0).max(80.0));
    Rect::new(width - panel_w - 8.0, CONTENT_TOP, panel_w, rows * 32.0)
}

fn chrome_rects(width: f32) -> (Rect, Rect, Rect, Rect) {
    let y = TAB_H + 12.0;
    let size = 32.0;
    let address_w = (width - 136.0 - 96.0).max(40.0);
    (
        Rect::new(12.0, y, size, size),
        Rect::new(52.0, y, size, size),
        Rect::new(92.0, y, size, size),
        Rect::new(136.0, y, address_w, size),
    )
}

fn retrieve(url: &Url) -> Result<Fetched, String> {
    match url.scheme.as_str() {
        "about" => {
            let (html, status) = match url.path.as_str() {
                "mithu" | "home" | "" => (home_page(), "Mithu".to_string()),
                "blank" => ("<!DOCTYPE html><html><head><title>about:blank</title></head><body></body></html>".to_string(), "about:blank".to_string()),
                other => (error_page(url, &format!("unknown about page {other}")), format!("unknown about:{other}")),
            };
            Ok(Fetched {
                url: url.clone(),
                html,
                status,
            })
        }
        "file" => {
            let path = url.path.clone();
            let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
            Ok(Fetched {
                url: url.clone(),
                html: bytes_to_html(url, Some("text/html"), &bytes),
                status: "file".to_string(),
            })
        }
        "http" | "https" => {
            let response = get(url).map_err(|error| error.to_string())?;
            if response.body.is_empty() && response.status >= 400 {
                return Err(format!("HTTP {}", response.status));
            }
            let status = format!("{} {}", response.status, response.url.to_display());
            let html = bytes_to_html(&response.url, response.content_type(), &response.body);
            Ok(Fetched {
                url: response.url,
                html,
                status,
            })
        }
        other => Err(format!("Mithu cannot open {other} URLs")),
    }
}

fn author_css(document: &Document, base: &Url) -> String {
    let mut author = String::new();
    for sheet in &document.stylesheets {
        author.push_str(sheet);
        author.push('\n');
    }
    for href in document.stylesheet_links.iter().take(6) {
        if let Some(css) = fetch_text(base, href)
            && css.len() <= 500_000
        {
            author.push_str(&css);
            author.push('\n');
        }
    }
    author
}

fn load_images(document: &Document, base: &Url) -> HashMap<NodeId, Arc<Bitmap>> {
    let mut images = HashMap::new();
    for id in (0..document.nodes.len()).map(NodeId) {
        if images.len() >= 16 || document.element_name(id) != Some("img") {
            continue;
        }
        let Some(src) = document.attr(id, "src") else {
            continue;
        };
        let bitmap = if src.starts_with("data:") {
            mithu_image::decode_data_url(src)
        } else {
            fetch_bytes(base, src).and_then(|bytes| mithu_image::decode(&bytes))
        };
        if let Some(bitmap) = bitmap {
            images.insert(id, Arc::new(bitmap));
        }
    }
    images
}

fn nearest(document: &Document, id: NodeId, matches: fn(&Document, NodeId) -> bool) -> Option<NodeId> {
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
            let kind = document.attr(id, "type").unwrap_or("text").to_ascii_lowercase();
            matches!(kind.as_str(), "text" | "search" | "email" | "password" | "url" | "tel" | "number")
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
        Some("button") => document.attr(id, "type").unwrap_or("submit").eq_ignore_ascii_case("submit"),
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

fn fetch_bytes(base: &Url, href: &str) -> Option<Vec<u8>> {
    let url = base.resolve(href).ok()?;
    match url.scheme.as_str() {
        "http" | "https" => {
            let response = get(&url).ok()?;
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
    let url = base.resolve(href).ok()?;
    match url.scheme.as_str() {
        "http" | "https" => {
            let response = get(&url).ok()?;
            if response.status >= 400 {
                return None;
            }
            Some(response.text())
        }
        "file" => std::fs::read_to_string(url.path).ok(),
        _ => None,
    }
}

fn bytes_to_html(url: &Url, content_type: Option<&str>, body: &[u8]) -> String {
    let mime = content_type.unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
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
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => query.push(*byte as char),
            b' ' => query.push('+'),
            _ => query.push_str(&format!("%{byte:02X}")),
        }
    }
    Url::parse(&format!("https://duckduckgo.com/?q={query}")).expect("search url")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn error_page(url: &Url, message: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><title>Load failed</title></head><body><h1>This page did not load</h1><p><b>{}</b></p><p>{}</p><p><a href=\"about:mithu\">Back to Mithu</a></p></body></html>",
        escape(&url.to_display()),
        escape(message)
    )
}

fn home_page() -> String {
    r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Mithu</title>
<style>
  body { margin: 0; background: #f3efe6; color: #1d1a16; }
  .hero { background: #16342f; color: #f6f7f4; padding: 36px 40px; }
  h1 { margin-top: 0; margin-bottom: 0; font-size: 40px; color: #f6f7f4; }
  .sub { color: #d5e4df; font-size: 18px; }
  .wrap { padding-left: 28px; padding-right: 28px; padding-bottom: 28px; }
  .card { background: #fffdf8; padding-left: 20px; padding-right: 20px; padding-bottom: 8px; margin-top: 16px; }
  a { color: #0b57d0; }
</style>
</head>
<body>
  <div class="hero">
    <h1>Mithu</h1>
    <p class="sub">Lightweight Rust browser. HTML, CSS, layout, and paint are written in this repo.</p>
  </div>
  <div class="wrap">
    <div class="card">
      <h2>Open a page</h2>
      <p>Type a URL in the address bar and press Enter. Click a link to open it.</p>
      <p>Mithu is named for a rose-ringed parakeet who was part of a family, and who is gone.</p>
      <p><a href="https://example.com/">example.com</a></p>
      <p><a href="https://info.cern.ch/">The first website</a></p>
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
    fn home_frame_has_chrome_and_page() {
        let mut browser = Browser::new();
        let frame = browser.render(800.0, 600.0);
        assert_eq!(frame.len(), 800 * 600);
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
        });
        browser.resize(800.0, 600.0);
        let pic = browser.page().document.get_element_by_id("pic").unwrap();
        let laid = find_box(&browser.page().layout.root, pic).expect("image box");
        assert!(laid.image.is_some());
        let x = (laid.rect.x + laid.rect.width / 2.0).floor() as usize;
        let y = (laid.rect.y + laid.rect.height / 2.0 + CONTENT_TOP).floor() as usize;
        let frame = browser.render(800.0, 600.0);
        assert_eq!(frame[y * 800 + x], 0x00FF_0000, "pixel {x},{y} is {:06x}", frame[y * 800 + x]);
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
        assert!(browser.page().scroll > 400.0, "scroll {}", browser.page().scroll);
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
            html: "<div style=\"width:100px;height:40px;background:#ff0000;margin:0\"></div>".to_string(),
            status: "test".to_string(),
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
        assert_eq!(frame[y * 800 + 40], 0x00FF_0000, "scaled pixel {:06x}", frame[y * 800 + 40]);
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
        assert_eq!(browser.title(), "Mithu");
        assert!(!browser.bookmarks_open());
        let (star, _) = bookmark_buttons(800.0);
        let frame = browser.render(800.0, 600.0);
        let sx = star.x as usize + 4;
        let sy = star.y as usize + 4;
        assert_eq!(frame[sy * 800 + sx], STAR.to_u32());
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
        let index = items.iter().position(|item| item.url.to_display() == "about:mithu").expect("suggestion");
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
        let new_tab = menu.items.iter().position(|(label, _)| label == "Open in new tab").expect("item");
        let bounds = menu_bounds(menu, 800.0, 600.0);
        browser.pointer_down(bounds.x + 12.0, bounds.y + new_tab as f32 * 32.0 + 10.0);
        assert_eq!(browser.tab_count(), 2);
        assert_eq!(browser.location(), "about:blank");
        assert!(!browser.menu_open());
        browser.select_tab(0);
        assert_eq!(browser.location(), "about:test");
        browser.context_menu(20.0, CONTENT_TOP + 20.0);
        let menu = browser.menu.as_ref().expect("menu");
        let reload = menu.items.iter().position(|(label, _)| label == "Reload").expect("reload");
        let bounds = menu_bounds(menu, 800.0, 600.0);
        browser.pointer_down(bounds.x + 12.0, bounds.y + reload as f32 * 32.0 + 10.0);
        assert_eq!(browser.location(), "about:test");
    }

    fn click_id(browser: &mut Browser, id: &str) {
        let (x, y) = {
            let page = browser.page();
            let node = page.document.get_element_by_id(id).unwrap();
            if let Some(laid) = find_box(&page.layout.root, node) {
                (laid.rect.x + laid.rect.width / 2.0, laid.rect.y + laid.rect.height / 2.0)
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
