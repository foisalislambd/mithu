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

pub const TOOLBAR_H: f32 = 56.0;
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

pub struct Browser {
    fonts: FontBook,
    viewport_w: f32,
    viewport_h: f32,
    history: Vec<Url>,
    index: usize,
    current: Url,
    base: Url,
    omnibox: String,
    cursor: usize,
    select_all: bool,
    pub omnibox_focused: bool,
    scroll: f32,
    title: String,
    status: String,
    document: Document,
    styles: Vec<ComputedStyle>,
    layout: LayoutTree,
    images: HashMap<NodeId, Arc<Bitmap>>,
    session: Option<Session>,
    focused: Option<NodeId>,
}

struct Fetched {
    url: Url,
    html: String,
    status: String,
}

impl Browser {
    pub fn new() -> Self {
        let mut browser = Self {
            fonts: FontBook::load(),
            viewport_w: 1200.0,
            viewport_h: 800.0,
            history: Vec::new(),
            index: 0,
            current: Url::parse("about:blank").expect("about:blank"),
            base: Url::parse("about:blank").expect("about:blank"),
            omnibox: String::new(),
            cursor: 0,
            select_all: false,
            omnibox_focused: false,
            scroll: 0.0,
            title: String::new(),
            status: String::new(),
            document: Document::new(),
            styles: Vec::new(),
            layout: layout_tree(&Document::new(), &[], 1200.0, &mithu_text::NaiveMetrics, &HashMap::new()),
            images: HashMap::new(),
            session: None,
            focused: None,
        };
        browser.navigate("about:mithu");
        browser
    }

    pub fn navigate(&mut self, input: &str) {
        self.navigate_url(interpret(input));
    }

    pub fn back(&mut self) {
        if self.index == 0 || self.history.is_empty() {
            return;
        }
        self.index -= 1;
        let url = self.history[self.index].clone();
        self.load_url(url);
    }

    pub fn forward(&mut self) {
        if self.index + 1 >= self.history.len() {
            return;
        }
        self.index += 1;
        let url = self.history[self.index].clone();
        self.load_url(url);
    }

    pub fn reload(&mut self) {
        let url = self.current.clone();
        self.load_url(url);
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.viewport_w = width.max(1.0);
        self.viewport_h = height.max(1.0);
        self.relayout();
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    pub fn scroll_by(&mut self, delta: f32) {
        self.scroll = (self.scroll + delta).clamp(0.0, self.max_scroll());
    }

    pub fn scroll_page(&mut self, pages: f32) {
        self.scroll_by(pages * self.page_height() * 0.9);
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) {
        if y >= self.viewport_h - STATUS_H {
            return;
        }
        if y < TOOLBAR_H {
            let (back, forward, reload, address) = chrome_rects(self.viewport_w);
            if back.contains(x, y) && self.can_back() {
                self.back();
            } else if forward.contains(x, y) && self.can_forward() {
                self.forward();
            } else if reload.contains(x, y) {
                self.reload();
            } else if address.contains(x, y) {
                self.focus_omnibox();
            }
            return;
        }
        self.omnibox_focused = false;
        self.select_all = false;
        let doc_y = y - TOOLBAR_H + self.scroll;
        self.activate(x, doc_y);
    }

    pub fn field_focused(&self) -> bool {
        self.focused.is_some() && !self.omnibox_focused
    }

    pub fn field_insert(&mut self, ch: char) {
        let Some(id) = self.focused else {
            return;
        };
        if let Some(session) = self.session.as_mut() {
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
        let Some(id) = self.focused else {
            return;
        };
        if let Some(session) = self.session.as_mut() {
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
        let Some(id) = self.focused else {
            return;
        };
        let submission = self.session.as_mut().and_then(|session| session.submission(id));
        self.pull_page();
        if let Some(submission) = submission {
            self.send_form(submission);
        }
    }

    pub fn blur_page(&mut self) {
        self.omnibox_focused = false;
        self.select_all = false;
        self.focused = None;
    }

    pub fn focus_omnibox(&mut self) {
        self.omnibox_focused = true;
        self.select_all = true;
    }

    pub fn blur_omnibox(&mut self) {
        self.omnibox_focused = false;
        self.select_all = false;
    }

    pub fn insert_char(&mut self, ch: char) {
        if ch.is_control() {
            return;
        }
        if self.select_all {
            self.omnibox.clear();
            self.cursor = 0;
            self.select_all = false;
        }
        let mut chars: Vec<char> = self.omnibox.chars().collect();
        let index = self.cursor.min(chars.len());
        chars.insert(index, ch);
        self.cursor = index + 1;
        self.omnibox = chars.into_iter().collect();
    }

    pub fn backspace(&mut self) {
        if self.select_all {
            self.omnibox.clear();
            self.cursor = 0;
            self.select_all = false;
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let mut chars: Vec<char> = self.omnibox.chars().collect();
        let index = self.cursor.min(chars.len());
        if index == 0 {
            return;
        }
        chars.remove(index - 1);
        self.cursor = index - 1;
        self.omnibox = chars.into_iter().collect();
    }

    pub fn move_cursor(&mut self, delta: i32) {
        let len = self.omnibox.chars().count();
        if self.select_all {
            self.select_all = false;
            self.cursor = if delta < 0 { 0 } else { len };
            return;
        }
        self.cursor = (self.cursor as i32 + delta).clamp(0, len as i32) as usize;
    }

    pub fn select_all_text(&mut self) {
        self.select_all = true;
    }

    pub fn submit_omnibox(&mut self) {
        let text = self.omnibox.clone();
        self.omnibox_focused = false;
        self.select_all = false;
        self.navigate(&text);
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn location(&self) -> String {
        self.current.to_display()
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
        if self.history.get(self.index) == Some(&url) {
            self.load_url(url);
            return;
        }
        if self.index + 1 < self.history.len() {
            self.history.truncate(self.index + 1);
        }
        self.history.push(url.clone());
        self.index = self.history.len() - 1;
        self.load_url(url);
    }

    fn load_url(&mut self, url: Url) {
        self.status = format!("Loading {}", url.to_display());
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
        let layout = layout_tree(&document, &styles, self.viewport_w, &self.fonts, &images);
        self.title = if document.title.is_empty() {
            fetched.url.to_display()
        } else {
            document.title.clone()
        };
        self.status = if logs.is_empty() {
            fetched.status
        } else {
            logs.last().cloned().unwrap_or(fetched.status)
        };
        self.current = fetched.url.clone();
        self.base = base;
        self.omnibox = fetched.url.to_display();
        self.cursor = self.omnibox.chars().count();
        self.omnibox_focused = false;
        self.select_all = false;
        self.scroll = 0.0;
        self.document = document;
        self.styles = styles;
        self.images = images;
        self.layout = layout;
        self.session = Some(session);
        self.focused = None;
    }

    fn relayout(&mut self) {
        self.layout = layout_tree(
            &self.document,
            &self.styles,
            self.viewport_w,
            &self.fonts,
            &self.images,
        );
    }

    fn activate(&mut self, x: f32, y: f32) {
        let hit = hit_test(&self.layout, x, y);
        let mut allow_default = true;
        let mut focus_field = None;
        let mut submission = None;
        if let Some(node) = hit
            && let Some(session) = self.session.as_mut()
        {
            allow_default = session.dispatch(node, "click");
            let document = session.document();
            if allow_default && is_text_field(&document, node) {
                focus_field = Some(node);
            } else if allow_default && is_checkbox(&document, node) {
                session.edit(|doc| toggle_checked(doc, node));
                session.dispatch(node, "change");
            } else if allow_default && is_submit(&document, node) {
                submission = session.submission(node);
            }
        }
        self.pull_page();
        self.focused = focus_field;
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
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.edit(|doc| doc.refresh_metadata());
        let document = session.document();
        let logs = session.logs();
        let author = author_css(&document, &self.base);
        self.styles = style_document(&document, &author);
        self.images = load_images(&document, &self.base);
        if let Some(log) = logs.last() {
            self.status = log.clone();
        }
        if !document.title.is_empty() {
            self.title = document.title.clone();
        }
        self.document = document;
        self.relayout();
    }

    fn send_form(&mut self, submission: Submission) {
        let mut url = if submission.action.is_empty() {
            self.current.clone()
        } else {
            self.base
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
        let url = self.base.resolve(&href).unwrap_or_else(|_| interpret(&href));
        self.navigate_url(url);
    }

    fn hit_link(&self, x: f32, y: f32) -> Option<String> {
        self.layout.links.iter().rev().find(|link| link.rect.contains(x, y)).map(|link| link.href.clone())
    }

    fn can_back(&self) -> bool {
        self.index > 0
    }

    fn can_forward(&self) -> bool {
        self.index + 1 < self.history.len()
    }

    fn page_height(&self) -> f32 {
        (self.viewport_h - TOOLBAR_H - STATUS_H).max(1.0)
    }

    fn max_scroll(&self) -> f32 {
        (self.layout.content_height - self.page_height()).max(0.0)
    }

    fn paint(&self, width: u32, height: u32) -> Vec<u32> {
        let mut pixels = vec![self.layout.background.to_u32(); width as usize * height as usize];
        let clip = Rect::new(0.0, TOOLBAR_H, width as f32, (height as f32 - TOOLBAR_H - STATUS_H).max(1.0));
        {
            let mut canvas = Canvas {
                pixels: &mut pixels,
                width: width as i32,
                height: height as i32,
            };
            paint_layout(&mut canvas, &self.layout, &self.fonts, clip, self.scroll, TOOLBAR_H);
            self.paint_chrome(&mut canvas);
        }
        pixels
    }

    fn paint_chrome(&self, canvas: &mut Canvas<'_>) {
        let width = canvas.width as f32;
        let height = canvas.height as f32;
        let full = Rect::new(0.0, 0.0, width, height);
        canvas.fill(Rect::new(0.0, 0.0, width, TOOLBAR_H), TOOLBAR, full);
        let (back, forward, reload, address) = chrome_rects(width);
        self.paint_button(canvas, back, "<", self.can_back());
        self.paint_button(canvas, forward, ">", self.can_forward());
        self.paint_button(canvas, reload, "R", true);
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
        let prefix: String = self.omnibox.chars().take(self.cursor).collect();
        let prefix_w = self.fonts.width(&prefix, size, false, false);
        let full_w = self.fonts.width(&self.omnibox, size, false, false);
        let mut shift = 0.0;
        if full_w > text_clip.width {
            shift = (prefix_w - text_clip.width + 12.0).max(0.0);
        }
        let text_x = text_clip.x - shift;
        let baseline = address.y + 22.0;
        if self.omnibox_focused && self.select_all && !self.omnibox.is_empty() {
            canvas.fill(Rect::new(text_x - 2.0, address.y + 6.0, full_w + 4.0, 22.0), SELECTION, text_clip);
        }
        canvas.text(
            &self.fonts,
            text_x,
            baseline,
            &self.omnibox,
            size,
            Color::rgb(28, 26, 22),
            false,
            false,
            false,
            text_clip,
        );
        if self.omnibox_focused && !self.select_all {
            canvas.fill(Rect::new(text_x + prefix_w, address.y + 8.0, 1.5, 18.0), Color::rgb(22, 52, 47), text_clip);
        }
        canvas.fill(Rect::new(0.0, height - STATUS_H, width, STATUS_H), STATUS_BG, full);
        let status = if self.title.is_empty() {
            self.status.clone()
        } else {
            format!("{}  —  {}", self.title, self.status)
        };
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

fn chrome_rects(width: f32) -> (Rect, Rect, Rect, Rect) {
    let y = 12.0;
    let size = 32.0;
    (
        Rect::new(12.0, y, size, size),
        Rect::new(52.0, y, size, size),
        Rect::new(92.0, y, size, size),
        Rect::new(136.0, y, (width - 148.0).max(40.0), size),
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
        assert_eq!(frame[10], TOOLBAR.to_u32());
        let page = frame[400 * 800 + 20];
        assert_ne!(page, TOOLBAR.to_u32());
    }

    #[test]
    fn clicking_a_link_navigates() {
        let mut browser = Browser::new();
        let test_url = Url::parse("about:test").unwrap();
        browser.history = vec![test_url.clone()];
        browser.index = 0;
        browser.install(Fetched {
            url: Url::parse("about:test").unwrap(),
            html: "<a href=\"about:blank\">Go there now</a>".to_string(),
            status: "test".to_string(),
        });
        browser.resize(800.0, 600.0);
        let link = browser
            .layout
            .links
            .iter()
            .find(|link| link.href.contains("about:blank"))
            .expect("link")
            .rect;
        browser.pointer_down(link.x + 2.0, link.y + 2.0 + TOOLBAR_H);
        assert_eq!(browser.location(), "about:blank");
        assert!(browser.can_back());
        browser.back();
        assert_eq!(browser.location(), "about:test");
    }
}
