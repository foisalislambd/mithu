//! Servo page inside the shared Mithu frame.
//!
//! The page (DOM, style, layout, script, network) is Servo. The tab strip,
//! toolbar, bookmarks, and address bar are painted by `mithu-ui`, then this
//! crate copies Servo's software framebuffer into the content rectangle.
//! No product logo is drawn.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use mithu_text::{FontBook, Glyph, glyph_top};
use servo::{
    Code, DevicePoint, ImeEvent, InputEvent, Key, KeyState, KeyboardEvent, LoadStatus, Location,
    Modifiers, MouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent, NamedKey,
    PixelFormat, RenderingContext, Servo, ServoBuilder, SoftwareRenderingContext, Theme, WebView,
    WebViewBuilder, WebViewDelegate, WebViewPoint, WheelDelta, WheelEvent, WheelMode,
};
use url::Url;

use mithu_ui::chrome;
pub use mithu_ui::{
    ADDRESS, ADDRESS_BORDER, BOOKMARK_BAR_H, CAPTION_W, INK, MUTED, PAGE, TAB_H, TOOLBAR_H, rgb,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameHit {
    Close,
    Minimize,
    Maximize,
    Drag,
}

#[derive(Clone)]
struct Delegate {
    frame: Rc<AtomicBool>,
    opened: Rc<RefCell<Vec<PendingTab>>>,
    shot: Rc<RefCell<Option<servo::RgbaImage>>>,
}

struct PendingTab {
    webview: WebView,
    context: Rc<SoftwareRenderingContext>,
}

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, webview: WebView) {
        webview.paint();
        self.frame.store(true, Ordering::SeqCst);
    }

    fn request_create_new(&self, _parent: WebView, request: servo::CreateNewWebViewRequest) {
        let size = PhysicalSize::new(800, 600);
        let Ok(context) = SoftwareRenderingContext::new(size) else {
            return;
        };
        let context = Rc::new(context);
        let dyn_ctx: Rc<dyn RenderingContext> = context.clone();
        let delegate: Rc<dyn WebViewDelegate> = Rc::new(self.clone());
        let webview = request.builder(dyn_ctx).delegate(delegate).build();
        webview.show();
        self.opened
            .borrow_mut()
            .push(PendingTab { webview, context });
    }
}

struct Tab {
    webview: WebView,
    context: Rc<SoftwareRenderingContext>,
    title: String,
    url: String,
    status: LoadStatus,
    zoom: f32,
}

struct Bookmark {
    title: String,
    url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Overlay {
    History,
    Downloads,
    Settings,
    NewTab,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    App,
    Context,
}

pub struct Browser {
    servo: Servo,
    delegate: Delegate,
    tabs: Vec<Tab>,
    active: usize,
    closed: Vec<String>,
    viewport_w: f32,
    viewport_h: f32,
    pub omnibox_focused: bool,
    omnibox: String,
    cursor: usize,
    select_all: bool,
    pub find_focused: bool,
    find_open: bool,
    find_query: String,
    bookmarks: Vec<Bookmark>,
    bookmarks_open: bool,
    visits: Vec<String>,
    downloads: Vec<String>,
    overlay: Option<Overlay>,
    menu: Option<MenuKind>,
    suggestion: Option<usize>,
    pointer: (f32, f32),
    pressed: bool,
    pressed_in_page: bool,
    link_status: String,
    tab_search: bool,
    extensions_open: bool,
    profile_open: bool,
    pending_url: Option<String>,
    dark: bool,
    fonts: FontBook,
    revision: u64,
    frame: Rc<AtomicBool>,
    status: String,
}

impl Browser {
    pub fn new() -> Self {
        match Self::try_new() {
            Ok(browser) => browser,
            Err(error) => panic!("{error}"),
        }
    }

    pub fn try_new() -> Result<Self, String> {
        install_crypto();
        let frame = Rc::new(AtomicBool::new(false));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let delegate = Delegate {
            frame: frame.clone(),
            opened,
            shot: Rc::new(RefCell::new(None)),
        };
        let mut preferences = servo::Preferences::default();
        preferences.network_http_proxy_uri = String::new();
        preferences.network_https_proxy_uri = String::new();
        let servo = ServoBuilder::default().preferences(preferences).build();
        servo.setup_logging();
        let mut browser = Self {
            servo,
            delegate,
            tabs: Vec::new(),
            active: 0,
            closed: Vec::new(),
            viewport_w: 1280.0,
            viewport_h: 800.0,
            omnibox_focused: false,
            omnibox: String::new(),
            cursor: 0,
            select_all: false,
            find_focused: false,
            find_open: false,
            find_query: String::new(),
            bookmarks: Vec::new(),
            bookmarks_open: true,
            visits: Vec::new(),
            downloads: Vec::new(),
            overlay: None,
            menu: None,
            suggestion: None,
            pointer: (-1.0, -1.0),
            pressed: false,
            pressed_in_page: false,
            link_status: String::new(),
            tab_search: false,
            extensions_open: false,
            profile_open: false,
            pending_url: None,
            dark: false,
            fonts: FontBook::load(),
            revision: 1,
            frame,
            status: String::new(),
        };
        browser.open_tab(start_url())?;
        browser.overlay = Some(Overlay::NewTab);
        browser.omnibox = "about:newtab".into();
        browser.spin();
        Ok(browser)
    }

    pub fn set_cooperative_scripts(&mut self, _on: bool) {}

    pub fn follow_os_theme(&mut self) {
        self.apply_theme();
    }

    pub fn chrome_animating(&self) -> bool {
        false
    }

    pub fn content_revision(&self) -> u64 {
        self.revision
    }

    pub fn status_line(&self) -> String {
        self.status.clone()
    }

    pub fn script_logs(&self) -> Vec<String> {
        Vec::new()
    }

    pub fn title(&self) -> String {
        self.tabs
            .get(self.active)
            .map(|tab| tab.title.clone())
            .unwrap_or_default()
    }

    pub fn load_settled(&self) -> bool {
        if self.overlay.is_some() {
            return true;
        }
        let done = matches!(
            self.tabs.get(self.active).map(|tab| tab.status),
            Some(LoadStatus::Complete | LoadStatus::Stopped)
        );
        let Some(want) = &self.pending_url else {
            return done;
        };
        let have = self
            .tabs
            .get(self.active)
            .map(|tab| tab.url.as_str())
            .unwrap_or("");
        if want.starts_with("http") {
            return done && have.starts_with("http") && self.frame.load(Ordering::SeqCst);
        }
        done && same_url(have, want)
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.viewport_w = width.max(1.0);
        self.viewport_h = height.max(1.0);
        self.resize_active();
    }

    pub fn navigate(&mut self, input: &str) {
        let url = parse_location(input);
        self.menu = None;
        self.omnibox_focused = false;
        if let Some(overlay) = overlay_for(&url) {
            self.overlay = Some(overlay);
            self.omnibox = url.to_string();
            self.bump();
            return;
        }
        self.overlay = None;
        self.frame.store(false, Ordering::SeqCst);
        self.pending_url = Some(url.to_string());
        self.wait_for_context();
        self.tabs[self.active].webview.load(url.clone());
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(500) && !self.load_settled() {
            self.spin();
            std::thread::sleep(Duration::from_millis(1));
        }
        if !self.load_settled() {
            // LoadUrl is dropped when the browsing context is not registered yet.
            self.tabs[self.active].webview.load(url);
            self.spin();
        }
    }

    fn wait_for_context(&mut self) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            self.spin();
            let ready = self
                .tabs
                .get(self.active)
                .is_some_and(|tab| tab.webview.url().is_some());
            if ready {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn back(&mut self) {
        if self.overlay.take().is_some() {
            self.sync_active();
            self.bump();
            return;
        }
        if self.tabs[self.active].webview.can_go_back() {
            self.pending_url = None;
            let _ = self.tabs[self.active].webview.go_back(1);
            self.spin();
        }
    }

    pub fn forward(&mut self) {
        if self.tabs[self.active].webview.can_go_forward() {
            self.pending_url = None;
            let _ = self.tabs[self.active].webview.go_forward(1);
            self.spin();
        }
    }

    pub fn reload(&mut self) {
        if self.overlay.is_some() {
            return;
        }
        if let Some(tab) = self.tabs.get(self.active) {
            self.pending_url = Some(tab.url.clone());
        }
        self.tabs[self.active].webview.reload();
        self.spin();
    }

    pub fn new_tab(&mut self) {
        if let Err(error) = self.open_tab(Url::parse("about:blank").expect("about:blank")) {
            self.status = error;
            return;
        }
        self.overlay = Some(Overlay::NewTab);
        self.omnibox = "about:newtab".into();
        self.omnibox_focused = false;
        self.tab_search = false;
        self.spin();
    }

    pub fn close_tab(&mut self) {
        if self.tabs.len() <= 1 {
            self.navigate("about:blank");
            return;
        }
        let url = self.tabs[self.active].url.clone();
        if !url.is_empty() {
            self.closed.push(url);
        }
        self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        self.resize_active();
        self.sync_active();
        self.bump();
    }

    pub fn reopen_closed_tab(&mut self) {
        let Some(url) = self.closed.pop() else {
            return;
        };
        self.navigate(&url);
    }

    pub fn next_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active = (self.active + 1) % self.tabs.len();
        self.resize_active();
        self.sync_active();
        self.bump();
    }

    pub fn previous_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
        self.resize_active();
        self.sync_active();
        self.bump();
    }

    pub fn zoom_in(&mut self) {
        self.set_zoom(self.tabs[self.active].zoom * 1.1);
    }

    pub fn zoom_out(&mut self) {
        self.set_zoom(self.tabs[self.active].zoom / 1.1);
    }

    pub fn zoom_reset(&mut self) {
        self.set_zoom(1.0);
    }

    pub fn toggle_bookmark(&mut self) {
        let url = self.location();
        if url.is_empty() {
            return;
        }
        if let Some(index) = self.bookmarks.iter().position(|mark| mark.url == url) {
            self.bookmarks.remove(index);
        } else {
            self.bookmarks.push(Bookmark {
                title: self.title(),
                url,
            });
        }
        self.bookmarks_open = true;
        self.bump();
    }

    pub fn toggle_bookmark_list(&mut self) {
        self.bookmarks_open = !self.bookmarks_open;
        self.bump();
    }

    pub fn bookmarks_open(&self) -> bool {
        self.bookmarks_open
    }

    pub fn close_bookmarks(&mut self) {
        self.bookmarks_open = false;
        self.bump();
    }

    pub fn open_find(&mut self) {
        self.find_open = true;
        self.find_focused = true;
        self.omnibox_focused = false;
        self.bump();
    }

    pub fn close_find(&mut self) {
        self.find_open = false;
        self.find_focused = false;
        self.bump();
    }

    pub fn find_is_open(&self) -> bool {
        self.find_open
    }

    pub fn find_insert(&mut self, ch: char) {
        self.find_query.push(ch);
        self.find_next();
    }

    pub fn find_backspace(&mut self) {
        self.find_query.pop();
        self.bump();
    }

    pub fn find_next(&mut self) {
        self.find_go(false);
    }

    pub fn find_previous(&mut self) {
        self.find_go(true);
    }

    pub fn focus_omnibox(&mut self) {
        self.omnibox = self.location();
        self.cursor = self.omnibox.chars().count();
        self.select_all = true;
        self.omnibox_focused = true;
        self.find_focused = false;
        self.suggestion = None;
        self.tabs[self.active].webview.set_focused(false);
        self.bump();
    }

    pub fn blur_page(&mut self) {
        self.omnibox_focused = false;
        self.find_focused = false;
        self.menu = None;
        self.tabs[self.active].webview.set_focused(true);
        self.bump();
    }

    pub fn insert_char(&mut self, ch: char) {
        if self.select_all {
            self.omnibox.clear();
            self.cursor = 0;
            self.select_all = false;
        }
        let byte = self.byte_at(self.cursor);
        self.omnibox.insert(byte, ch);
        self.cursor += 1;
        self.refresh_suggestions();
        self.bump();
    }

    pub fn backspace(&mut self) {
        if self.select_all {
            self.omnibox.clear();
            self.cursor = 0;
            self.select_all = false;
            self.bump();
            return;
        }
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let byte = self.byte_at(self.cursor);
        self.omnibox.remove(byte);
        self.refresh_suggestions();
        self.bump();
    }

    pub fn select_all_text(&mut self) {
        self.select_all = true;
        self.bump();
    }

    pub fn move_cursor(&mut self, delta: i32) {
        self.select_all = false;
        let len = self.omnibox.chars().count() as i32;
        self.cursor = (self.cursor as i32 + delta).clamp(0, len) as usize;
        self.bump();
    }

    pub fn move_suggestion(&mut self, delta: i32) {
        let count = self.suggestions().len() as i32;
        if count == 0 {
            return;
        }
        let current = self.suggestion.unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, count - 1) as usize;
        self.suggestion = Some(next);
        if let Some(url) = self.suggestions().get(next) {
            self.omnibox = url.clone();
            self.cursor = self.omnibox.chars().count();
        }
        self.bump();
    }

    pub fn submit_omnibox(&mut self) {
        if let Some(index) = self.suggestion {
            if let Some(url) = self.suggestions().get(index) {
                self.navigate(url);
                return;
            }
        }
        let text = self.omnibox.clone();
        self.navigate(&text);
    }

    pub fn field_focused(&self) -> bool {
        false
    }

    pub fn submit_field(&mut self) {}

    pub fn field_backspace(&mut self) {}

    pub fn field_insert(&mut self, _ch: char) {}

    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    pub fn close_menu(&mut self) {
        self.menu = None;
        self.bump();
    }

    pub fn context_menu(&mut self, x: f32, y: f32) {
        self.pointer = (x, y);
        self.menu = Some(MenuKind::Context);
        self.bump();
    }

    pub fn pointer_move(&mut self, x: f32, y: f32) {
        self.pointer = (x, y);
        if let Some((px, py)) = self.content_point(x, y) {
            if self.overlay.is_none() {
                let point = WebViewPoint::Device(DevicePoint::new(px, py));
                self.tabs[self.active]
                    .webview
                    .notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(point)));
            }
        }
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) {
        self.pointer = (x, y);
        self.pressed = true;
        match self.click_target(x, y) {
            Click::Frame(_) => {}
            Click::Tab(index) => {
                self.tab_search = false;
                self.active = index;
                let keep_ntp = self.overlay == Some(Overlay::NewTab)
                    && self
                        .tabs
                        .get(index)
                        .is_some_and(|tab| tab.url.is_empty() || tab.url.starts_with("about:"));
                if !keep_ntp {
                    self.overlay = None;
                }
                self.resize_active();
                self.sync_active();
                self.bump();
            }
            Click::CloseTab(index) => {
                self.active = index;
                self.close_tab();
            }
            Click::NewTab => self.new_tab(),
            Click::TabSearch => {
                self.tab_search = !self.tab_search;
                self.menu = None;
                self.extensions_open = false;
                self.profile_open = false;
                self.bump();
            }
            Click::Back => self.back(),
            Click::Forward => self.forward(),
            Click::Home => self.navigate("about:newtab"),
            Click::Reload => self.reload(),
            Click::Omnibox => self.focus_omnibox(),
            Click::Star => self.toggle_bookmark(),
            Click::Extensions => {
                self.extensions_open = !self.extensions_open;
                self.menu = None;
                self.tab_search = false;
                self.profile_open = false;
                self.bump();
            }
            Click::Profile => {
                self.profile_open = !self.profile_open;
                self.menu = None;
                self.tab_search = false;
                self.extensions_open = false;
                self.bump();
            }
            Click::MenuButton => {
                self.menu = if self.menu == Some(MenuKind::App) {
                    None
                } else {
                    Some(MenuKind::App)
                };
                self.tab_search = false;
                self.extensions_open = false;
                self.profile_open = false;
                self.bump();
            }
            Click::Suggestion(index) => {
                self.suggestion = Some(index);
                self.submit_omnibox();
            }
            Click::Panel => {}
            Click::Bookmark(index) => {
                if let Some(mark) = self.bookmarks.get(index) {
                    let url = mark.url.clone();
                    self.navigate(&url);
                }
            }
            Click::MenuItem(item) | Click::ContextItem(item) => {
                self.menu = None;
                self.run_item(item);
            }
            Click::Theme => {
                self.dark = !self.dark;
                self.apply_theme();
                self.bump();
            }
            Click::Content => {
                self.menu = None;
                self.omnibox_focused = false;
                self.find_focused = false;
                if self.overlay.is_some() {
                    return;
                }
                if let Some((px, py)) = self.content_point(x, y) {
                    let point = WebViewPoint::Device(DevicePoint::new(px, py));
                    self.tabs[self.active].webview.set_focused(true);
                    self.tabs[self.active]
                        .webview
                        .notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                            MouseButtonAction::Down,
                            MouseButton::Primary,
                            point,
                        )));
                    self.pressed_in_page = true;
                }
            }
            Click::None => {
                self.menu = None;
                self.tab_search = false;
                self.extensions_open = false;
                self.profile_open = false;
                self.bump();
            }
        }
    }

    pub fn pointer_up(&mut self) {
        if self.pressed {
            self.pressed = false;
            self.bump();
        }
        if !self.pressed_in_page {
            return;
        }
        self.pressed_in_page = false;
        if let Some((px, py)) = self.content_point(self.pointer.0, self.pointer.1) {
            let point = WebViewPoint::Device(DevicePoint::new(px, py));
            self.tabs[self.active]
                .webview
                .notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
                    MouseButtonAction::Up,
                    MouseButton::Primary,
                    point,
                )));
        }
    }

    pub fn frame_hit(&self, x: f32, y: f32) -> Option<FrameHit> {
        match self.click_target(x, y) {
            Click::Frame(hit) => Some(hit),
            _ => None,
        }
    }

    pub fn scroll_by(&mut self, dy: f32) {
        self.wheel(0.0, -f64::from(dy));
    }

    pub fn scroll_page(&mut self, pages: f32) {
        let height = (self.viewport_h - self.content_top()).max(1.0);
        self.wheel(0.0, -f64::from(pages * height));
    }

    pub fn send_key(&mut self, kind: KeyKind, pressed: bool, ctrl: bool, alt: bool, shift: bool) {
        if self.omnibox_focused || self.find_focused {
            return;
        }
        if self.overlay.is_some() {
            return;
        }
        let state = if pressed {
            KeyState::Down
        } else {
            KeyState::Up
        };
        let (key, code) = kind.to_servo();
        let mut modifiers = Modifiers::empty();
        if ctrl {
            modifiers.insert(Modifiers::CONTROL);
        }
        if alt {
            modifiers.insert(Modifiers::ALT);
        }
        if shift {
            modifiers.insert(Modifiers::SHIFT);
        }
        let event = KeyboardEvent::new_without_event(
            state,
            key,
            code,
            Location::Standard,
            modifiers,
            false,
            false,
        );
        self.tabs[self.active]
            .webview
            .notify_input_event(InputEvent::Keyboard(event));
    }

    pub fn ime_commit(&mut self, text: &str) {
        if self.omnibox_focused || self.find_focused || text.is_empty() {
            return;
        }
        self.tabs[self.active]
            .webview
            .notify_input_event(InputEvent::Ime(ImeEvent::Commit(text.to_string())));
    }

    pub fn pump_scripts(&mut self, budget: Duration) -> bool {
        let started = Instant::now();
        self.spin();
        while started.elapsed() < budget && !self.load_settled() {
            self.spin();
            std::thread::sleep(Duration::from_millis(1));
        }
        self.frame.store(false, Ordering::SeqCst);
        !self.load_settled()
            || self
                .tabs
                .get(self.active)
                .is_some_and(|tab| tab.webview.animating())
    }

    pub fn animation_frames_pending(&self) -> bool {
        self.tabs
            .get(self.active)
            .is_some_and(|tab| tab.webview.animating())
    }

    pub fn advance_frame(&mut self, _ms: u64) -> bool {
        let before = self.revision;
        self.spin();
        self.revision != before || self.frame.swap(false, Ordering::SeqCst)
    }

    pub fn advance_limited(&mut self, _ms: u64, _url: Option<&str>, _steps: u32) -> bool {
        self.spin();
        true
    }

    pub fn advance_timers(&mut self, _ms: u64) -> bool {
        let before = self.revision;
        self.spin();
        self.revision != before
    }

    pub fn next_timer_ms(&self) -> Option<u64> {
        if self.animation_frames_pending() || !self.load_settled() {
            Some(16)
        } else {
            None
        }
    }

    pub fn render(&mut self, width: f32, height: f32) -> Vec<u32> {
        self.resize(width, height);
        self.spin();
        let w = width.round().max(1.0) as u32;
        let h = height.round().max(1.0) as u32;
        let mut pixels = vec![PAGE; (w as usize) * (h as usize)];
        if self.overlay.is_none() {
            self.blit_page(&mut pixels, w);
        } else {
            self.draw_overlay(&mut pixels, w, h);
        }
        self.draw_chrome(&mut pixels, w, h);
        pixels
    }

    fn open_tab(&mut self, url: Url) -> Result<(), String> {
        let size = self.content_physical();
        let context = Rc::new(software_context(size)?);
        let _ = context.make_current();
        let dyn_ctx: Rc<dyn RenderingContext> = context.clone();
        let delegate: Rc<dyn WebViewDelegate> = Rc::new(self.delegate.clone());
        let webview = WebViewBuilder::new(&self.servo, dyn_ctx)
            .delegate(delegate)
            .url(url)
            .hidpi_scale_factor(euclid_scale())
            .build();
        webview.show();
        webview.set_focused(true);
        if self.dark {
            webview.notify_theme_change(Theme::Dark);
        }
        self.tabs.push(Tab {
            webview,
            context,
            title: "New Tab".into(),
            url: "about:blank".into(),
            status: LoadStatus::Started,
            zoom: 1.0,
        });
        self.active = self.tabs.len() - 1;
        Ok(())
    }

    fn adopt_pending(&mut self) {
        let pending = std::mem::take(&mut *self.delegate.opened.borrow_mut());
        for item in pending {
            item.webview.resize(self.content_physical());
            self.tabs.push(Tab {
                webview: item.webview,
                context: item.context,
                title: "New Tab".into(),
                url: String::new(),
                status: LoadStatus::Started,
                zoom: 1.0,
            });
            self.active = self.tabs.len() - 1;
            self.bump();
        }
    }

    fn spin(&mut self) {
        if let Some(tab) = self.tabs.get(self.active) {
            let _ = tab.context.make_current();
        }
        self.servo.spin_event_loop();
        self.adopt_pending();
        self.sync_active();
        if self.frame.swap(false, Ordering::SeqCst) {
            self.bump();
        }
    }

    fn sync_active(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let status = tab.webview.load_status();
        let url = tab
            .webview
            .url()
            .map(|url| url.to_string())
            .unwrap_or_default();
        let title = tab
            .webview
            .page_title()
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| display_title(&url));
        let changed = tab.status != status || tab.url != url || tab.title != title;
        let link = tab.webview.status_text().unwrap_or_default();
        tab.status = status;
        if tab.url != url && url.starts_with("http") && !self.visits.contains(&url) {
            self.visits.push(url.clone());
        }
        tab.url = url;
        tab.title = title;
        self.link_status = link;
        if changed {
            self.bump();
        }
        if !self.omnibox_focused {
            if let Some(overlay) = self.overlay {
                self.omnibox = match overlay {
                    Overlay::History => "about:history".into(),
                    Overlay::Downloads => "about:downloads".into(),
                    Overlay::Settings => "about:settings".into(),
                    Overlay::NewTab => "about:newtab".into(),
                };
            } else if let Some(tab) = self.tabs.get(self.active) {
                self.omnibox = tab.url.clone();
            }
        }
        self.status = if self.overlay.is_some() {
            self.location()
        } else {
            match self.tabs.get(self.active).map(|tab| tab.status) {
                Some(LoadStatus::Started | LoadStatus::HeadParsed) => "Loading".into(),
                _ => self.location(),
            }
        };
        if self.load_settled() {
            self.pending_url = None;
        }
    }

    fn resize_active(&mut self) {
        if let Some(tab) = self.tabs.get(self.active) {
            tab.webview.resize(self.content_physical());
        }
    }

    fn content_physical(&self) -> PhysicalSize<u32> {
        let w = self.viewport_w.max(1.0) as u32;
        let h = (self.viewport_h - self.content_top()).max(1.0) as u32;
        PhysicalSize::new(w.max(1), h.max(1))
    }

    fn content_top(&self) -> f32 {
        let mut top = TAB_H + TOOLBAR_H;
        if self.bookmarks_open {
            top += BOOKMARK_BAR_H;
        }
        top
    }

    fn content_point(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let top = self.content_top();
        if y < top || x < 0.0 || x >= self.viewport_w {
            return None;
        }
        Some((x, y - top))
    }

    fn set_zoom(&mut self, zoom: f32) {
        let zoom = zoom.clamp(0.25, 5.0);
        let tab = &mut self.tabs[self.active];
        tab.zoom = zoom;
        tab.webview.set_page_zoom(zoom);
        self.bump();
    }

    fn apply_theme(&mut self) {
        let theme = if self.dark { Theme::Dark } else { Theme::Light };
        for tab in &self.tabs {
            tab.webview.notify_theme_change(theme);
        }
    }

    fn find_go(&mut self, back: bool) {
        if self.find_query.is_empty() || self.overlay.is_some() {
            return;
        }
        let query = self.find_query.replace('\\', "\\\\").replace('\'', "\\'");
        let script = format!("window.find('{query}', false, {back})");
        self.tabs[self.active]
            .webview
            .evaluate_javascript(script, |_| {});
        self.spin();
    }

    fn wheel(&mut self, dx: f64, dy: f64) {
        if self.overlay.is_some() {
            return;
        }
        let (px, py) = self
            .content_point(self.pointer.0, self.pointer.1)
            .unwrap_or((0.0, 0.0));
        let delta = WheelDelta {
            x: dx,
            y: dy,
            z: 0.0,
            mode: WheelMode::DeltaPixel,
        };
        let event = WheelEvent::new(
            delta,
            WebViewPoint::Device(DevicePoint::new(px, py)),
            Modifiers::empty(),
        );
        self.tabs[self.active]
            .webview
            .notify_input_event(InputEvent::Wheel(event));
    }

    fn location(&self) -> String {
        if let Some(overlay) = self.overlay {
            return match overlay {
                Overlay::History => "about:history".into(),
                Overlay::Downloads => "about:downloads".into(),
                Overlay::Settings => "about:settings".into(),
                Overlay::NewTab => "about:newtab".into(),
            };
        }
        self.tabs
            .get(self.active)
            .map(|tab| tab.url.clone())
            .unwrap_or_default()
    }

    fn run_item(&mut self, item: Item) {
        match item {
            Item::Back => self.back(),
            Item::Reload => self.reload(),
            Item::NewTab => self.new_tab(),
            Item::Bookmark => self.toggle_bookmark(),
            Item::History => self.navigate("about:history"),
            Item::Downloads => self.navigate("about:downloads"),
            Item::Settings => self.navigate("about:settings"),
        }
    }

    fn suggestions(&self) -> Vec<String> {
        if !self.omnibox_focused {
            return Vec::new();
        }
        let needle = self.omnibox.to_ascii_lowercase();
        let mut out = Vec::new();
        for mark in &self.bookmarks {
            if mark.url.to_ascii_lowercase().contains(&needle)
                || mark.title.to_ascii_lowercase().contains(&needle)
            {
                out.push(mark.url.clone());
            }
        }
        for url in self.visits.iter().rev() {
            if url.to_ascii_lowercase().contains(&needle) && !out.contains(url) {
                out.push(url.clone());
            }
            if out.len() >= 6 {
                break;
            }
        }
        out
    }

    fn refresh_suggestions(&mut self) {
        self.suggestion = if self.suggestions().is_empty() {
            None
        } else {
            Some(0)
        };
    }

    fn byte_at(&self, cursor: usize) -> usize {
        self.omnibox
            .char_indices()
            .nth(cursor)
            .map(|(index, _)| index)
            .unwrap_or(self.omnibox.len())
    }

    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn capture_page(&mut self) -> Option<servo::RgbaImage> {
        let done = Rc::new(std::cell::Cell::new(false));
        let slot = self.delegate.shot.clone();
        *slot.borrow_mut() = None;
        let done_cb = done.clone();
        let slot_cb = slot.clone();
        self.tabs[self.active]
            .webview
            .take_screenshot(None, move |result| {
                if let Ok(image) = result {
                    *slot_cb.borrow_mut() = Some(image);
                }
                done_cb.set(true);
            });
        let started = Instant::now();
        while !done.get() && started.elapsed() < Duration::from_secs(8) {
            self.spin();
            std::thread::sleep(Duration::from_millis(1));
        }
        slot.borrow_mut().take()
    }

    fn blit_page(&mut self, pixels: &mut [u32], width: u32) {
        let Some(image) = self.capture_page() else {
            return;
        };
        let top = self.content_top().round() as i32;
        let raw = image.as_raw();
        let iw = image.width() as i32;
        let ih = image.height() as i32;
        for y in 0..ih {
            let dest_y = top + y;
            if dest_y < 0 || dest_y >= self.viewport_h as i32 {
                continue;
            }
            for x in 0..iw {
                if x >= width as i32 {
                    break;
                }
                let index = ((y as u32 * image.width()) + x as u32) as usize * 4;
                if index + 3 >= raw.len() {
                    break;
                }
                let pixel = rgb(
                    raw[index] as u32,
                    raw[index + 1] as u32,
                    raw[index + 2] as u32,
                );
                pixels[(dest_y as u32 * width + x as u32) as usize] = pixel;
            }
        }
    }

    fn draw_overlay(&self, pixels: &mut [u32], width: u32, height: u32) {
        let top = self.content_top();
        fill_rect(
            pixels,
            width,
            height,
            0.0,
            top,
            self.viewport_w,
            self.viewport_h - top,
            PAGE,
        );
        let mut y = top + 36.0;
        match self.overlay {
            Some(Overlay::History) => {
                self.draw_text(pixels, width, height, 24.0, y, 22.0, INK, "History");
                y += 36.0;
                if self.visits.is_empty() {
                    self.draw_text(pixels, width, height, 24.0, y, 16.0, MUTED, "No pages yet.");
                }
                for url in self.visits.iter().rev().take(12) {
                    self.draw_text(pixels, width, height, 24.0, y, 15.0, INK, url);
                    y += 24.0;
                }
            }
            Some(Overlay::Downloads) => {
                self.draw_text(pixels, width, height, 24.0, y, 22.0, INK, "Downloads");
                y += 36.0;
                self.draw_text(
                    pixels,
                    width,
                    height,
                    24.0,
                    y,
                    15.0,
                    MUTED,
                    "libservo does not expose a download callback, so files are not saved yet.",
                );
                y += 28.0;
                for item in &self.downloads {
                    self.draw_text(pixels, width, height, 24.0, y, 15.0, INK, item);
                    y += 24.0;
                }
            }
            Some(Overlay::NewTab) | None => {}
            Some(Overlay::Settings) => {
                self.draw_text(pixels, width, height, 24.0, y, 22.0, INK, "Settings");
                y += 40.0;
                let label = if self.dark {
                    "Theme: dark. Click to use light."
                } else {
                    "Theme: light. Click to use dark."
                };
                fill_rect(pixels, width, height, 24.0, y, 360.0, 36.0, ADDRESS);
                stroke_rect(pixels, width, height, 24.0, y, 360.0, 36.0, ADDRESS_BORDER);
                self.draw_text(pixels, width, height, 36.0, y + 24.0, 15.0, INK, label);
            }
        }
    }

    fn draw_chrome(&self, pixels: &mut [u32], width: u32, height: u32) {
        let state = self.chrome_state();
        chrome::paint(pixels, width, height, &state);
    }

    fn chrome_state(&self) -> chrome::State<'_> {
        let tabs = self
            .tabs
            .iter()
            .map(|tab| chrome::TabPaint {
                title: if tab.title.is_empty() {
                    "New Tab".into()
                } else {
                    tab.title.clone()
                },
                favicon: tab.webview.favicon().map(|image| favicon_rgba(&image)),
            })
            .collect();
        let secure = if self.omnibox.starts_with("https://") {
            chrome::Secure::Https
        } else if self.omnibox.starts_with("http://") {
            chrome::Secure::Http
        } else {
            chrome::Secure::Internal
        };
        let location = self.location();
        let loading = self.overlay.is_none()
            && matches!(
                self.tabs.get(self.active).map(|tab| tab.status),
                Some(LoadStatus::Started | LoadStatus::HeadParsed)
            );
        let progress = match self.tabs.get(self.active).map(|tab| tab.status) {
            Some(LoadStatus::Started) => 0.35,
            Some(LoadStatus::HeadParsed) => 0.7,
            _ => 0.0,
        };
        let menu = match self.menu {
            Some(MenuKind::App) => Some(chrome::Popup::App),
            Some(MenuKind::Context) => Some(chrome::Popup::Context),
            None => None,
        };
        chrome::State {
            width: self.viewport_w,
            height: self.viewport_h,
            tabs,
            active: self.active,
            omnibox: self.omnibox.clone(),
            omnibox_focused: self.omnibox_focused,
            cursor: self.cursor,
            secure,
            bookmarked: self.bookmarks.iter().any(|mark| mark.url == location),
            bookmarks: self
                .bookmarks
                .iter()
                .map(|mark| (mark.title.clone(), mark.url.clone()))
                .collect(),
            bookmarks_open: self.bookmarks_open,
            can_back: self
                .tabs
                .get(self.active)
                .is_some_and(|tab| tab.webview.can_go_back()),
            can_forward: self
                .tabs
                .get(self.active)
                .is_some_and(|tab| tab.webview.can_go_forward()),
            loading,
            progress,
            link_status: self.link_status.clone(),
            pointer: self.pointer,
            pressed: self.pressed,
            ntp: self.overlay == Some(Overlay::NewTab),
            menu,
            menu_labels: menu_items(self.menu)
                .into_iter()
                .map(|item| item.label().to_string())
                .collect(),
            tab_search: self.tab_search,
            extensions: self.extensions_open,
            profile: self.profile_open,
            suggestions: if self.omnibox_focused {
                self.suggestions()
            } else {
                Vec::new()
            },
            suggestion: self.suggestion,
            find_open: self.find_open,
            find_query: self.find_query.clone(),
            fonts: &self.fonts,
        }
    }

    fn draw_text(
        &self,
        pixels: &mut [u32],
        width: u32,
        height: u32,
        x: f32,
        baseline: f32,
        size: f32,
        color: u32,
        text: &str,
    ) {
        let glyphs = self.fonts.shape(text, size, false, false, "");
        let mut pen = x;
        let max_x = self.viewport_w - 8.0;
        for glyph in glyphs {
            if pen > max_x {
                break;
            }
            blit_glyph(pixels, width, height, pen, baseline, &glyph, color);
            pen += glyph.advance;
        }
    }

    fn click_target(&self, x: f32, y: f32) -> Click {
        let state = self.chrome_state();
        match chrome::hit(&state, x, y) {
            chrome::Hit::Drag => Click::Frame(FrameHit::Drag),
            chrome::Hit::Minimize => Click::Frame(FrameHit::Minimize),
            chrome::Hit::Maximize => Click::Frame(FrameHit::Maximize),
            chrome::Hit::Close => Click::Frame(FrameHit::Close),
            chrome::Hit::Tab(index) => Click::Tab(index),
            chrome::Hit::CloseTab(index) => Click::CloseTab(index),
            chrome::Hit::NewTab => Click::NewTab,
            chrome::Hit::TabSearch => Click::TabSearch,
            chrome::Hit::Back => Click::Back,
            chrome::Hit::Forward => Click::Forward,
            chrome::Hit::Reload => Click::Reload,
            chrome::Hit::Home => Click::Home,
            chrome::Hit::Omnibox => Click::Omnibox,
            chrome::Hit::Star => Click::Star,
            chrome::Hit::Extensions => Click::Extensions,
            chrome::Hit::Profile => Click::Profile,
            chrome::Hit::Menu => Click::MenuButton,
            chrome::Hit::Bookmark(index) => Click::Bookmark(index),
            chrome::Hit::MenuItem(index) => {
                let items = menu_items(self.menu);
                match self.menu {
                    Some(MenuKind::Context) => items
                        .get(index)
                        .copied()
                        .map(Click::ContextItem)
                        .unwrap_or(Click::None),
                    _ => items
                        .get(index)
                        .copied()
                        .map(Click::MenuItem)
                        .unwrap_or(Click::None),
                }
            }
            chrome::Hit::Suggestion(index) => Click::Suggestion(index),
            chrome::Hit::Panel => Click::Panel,
            chrome::Hit::Content => {
                if let Some(Overlay::Settings) = self.overlay {
                    let top = self.content_top() + 76.0;
                    if x >= 24.0 && x <= 384.0 && y >= top && y <= top + 36.0 {
                        return Click::Theme;
                    }
                }
                Click::Content
            }
            chrome::Hit::None => Click::None,
        }
    }
}

#[derive(Clone, Copy)]
enum Click {
    Frame(FrameHit),
    Tab(usize),
    CloseTab(usize),
    NewTab,
    Back,
    Forward,
    Reload,
    Omnibox,
    Star,
    Home,
    TabSearch,
    Extensions,
    Profile,
    Suggestion(usize),
    Panel,
    MenuButton,
    Bookmark(usize),
    MenuItem(Item),
    ContextItem(Item),
    Theme,
    Content,
    None,
}

#[derive(Clone, Copy)]
enum Item {
    Back,
    Reload,
    NewTab,
    Bookmark,
    History,
    Downloads,
    Settings,
}

impl Item {
    fn label(self) -> &'static str {
        match self {
            Item::Back => "Back",
            Item::Reload => "Reload",
            Item::NewTab => "New tab",
            Item::Bookmark => "Bookmark this page",
            Item::History => "History",
            Item::Downloads => "Downloads",
            Item::Settings => "Settings",
        }
    }
}

fn menu_items(kind: Option<MenuKind>) -> Vec<Item> {
    match kind {
        Some(MenuKind::Context) => vec![Item::Back, Item::Reload, Item::NewTab, Item::Bookmark],
        Some(MenuKind::App) => vec![
            Item::NewTab,
            Item::History,
            Item::Downloads,
            Item::Settings,
            Item::Bookmark,
        ],
        None => Vec::new(),
    }
}

pub enum KeyKind {
    Char(char),
    Enter,
    Escape,
    Backspace,
    Tab,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Space,
}

impl KeyKind {
    fn to_servo(self) -> (Key, Code) {
        match self {
            KeyKind::Char(ch) => (Key::Character(ch.to_string()), Code::Unidentified),
            KeyKind::Enter => (Key::Named(NamedKey::Enter), Code::Enter),
            KeyKind::Escape => (Key::Named(NamedKey::Escape), Code::Escape),
            KeyKind::Backspace => (Key::Named(NamedKey::Backspace), Code::Backspace),
            KeyKind::Tab => (Key::Named(NamedKey::Tab), Code::Tab),
            KeyKind::Left => (Key::Named(NamedKey::ArrowLeft), Code::ArrowLeft),
            KeyKind::Right => (Key::Named(NamedKey::ArrowRight), Code::ArrowRight),
            KeyKind::Up => (Key::Named(NamedKey::ArrowUp), Code::ArrowUp),
            KeyKind::Down => (Key::Named(NamedKey::ArrowDown), Code::ArrowDown),
            KeyKind::PageUp => (Key::Named(NamedKey::PageUp), Code::PageUp),
            KeyKind::PageDown => (Key::Named(NamedKey::PageDown), Code::PageDown),
            KeyKind::Home => (Key::Named(NamedKey::Home), Code::Home),
            KeyKind::End => (Key::Named(NamedKey::End), Code::End),
            KeyKind::Space => (Key::Character(" ".into()), Code::Space),
        }
    }
}

fn install_crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

fn euclid_scale() -> euclid::Scale<f32, servo::DeviceIndependentPixel, servo::DevicePixel> {
    euclid::Scale::new(1.0)
}

/// Offscreen page context. Servo paints into this and Mithu copies the
/// pixels into the window, so the window itself is never a GL surface.
/// The first attempt is the requested size. If that context cannot be
/// created, a 64×64 surface is created and resized — the software fallback
/// when the GPU context fails.
fn software_context(size: PhysicalSize<u32>) -> Result<SoftwareRenderingContext, String> {
    let size = PhysicalSize::new(size.width.max(1), size.height.max(1));
    match SoftwareRenderingContext::new(size) {
        Ok(context) => Ok(context),
        Err(first) => match SoftwareRenderingContext::new(PhysicalSize::new(64, 64)) {
            Ok(context) => {
                context.resize(size);
                eprintln!("mithu: OpenGL context failed ({first:?}); using a software surface");
                Ok(context)
            }
            Err(second) => Err(format!(
                "the page renderer could not create an OpenGL context ({first:?}). \
                 The software surface fallback failed too ({second:?})"
            )),
        },
    }
}

fn start_url() -> Url {
    Url::parse("about:blank").expect("about:blank")
}

fn parse_location(input: &str) -> Url {
    mithu_ui::parse_location(input)
}

fn overlay_for(url: &Url) -> Option<Overlay> {
    if url.scheme() != "about" {
        return None;
    }
    match url.path() {
        "history" => Some(Overlay::History),
        "downloads" => Some(Overlay::Downloads),
        "settings" => Some(Overlay::Settings),
        "newtab" | "blank" | "mithu" | "home" => Some(Overlay::NewTab),
        _ => None,
    }
}

#[cfg(test)]
fn caption_hit(width: f32, x: f32) -> Option<FrameHit> {
    if x >= width - CAPTION_W {
        Some(FrameHit::Close)
    } else if x >= width - CAPTION_W * 2.0 {
        Some(FrameHit::Maximize)
    } else if x >= width - CAPTION_W * 3.0 {
        Some(FrameHit::Minimize)
    } else {
        None
    }
}

fn same_url(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}

fn display_title(url: &str) -> String {
    if url.is_empty() || url == "about:blank" {
        "New Tab".into()
    } else {
        url.to_string()
    }
}

fn favicon_rgba(image: &servo::Image) -> (u32, u32, Vec<u8>) {
    let data = image.data();
    let stride = match image.format {
        PixelFormat::RGBA8 | PixelFormat::BGRA8 => 4,
        PixelFormat::RGB8 => 3,
        PixelFormat::KA8 => 2,
        PixelFormat::K8 => 1,
    };
    let mut rgba = Vec::with_capacity((image.width * image.height * 4) as usize);
    for row in 0..image.height {
        for col in 0..image.width {
            let index = (row * image.width + col) as usize * stride;
            if index >= data.len() {
                return (image.width, image.height, rgba);
            }
            let (red, green, blue, alpha) = match image.format {
                PixelFormat::RGBA8 => (
                    data[index],
                    data[index + 1],
                    data[index + 2],
                    data.get(index + 3).copied().unwrap_or(255),
                ),
                PixelFormat::BGRA8 => (
                    data[index + 2],
                    data[index + 1],
                    data[index],
                    data.get(index + 3).copied().unwrap_or(255),
                ),
                PixelFormat::RGB8 => (data[index], data[index + 1], data[index + 2], 255),
                PixelFormat::K8 => (data[index], data[index], data[index], 255),
                PixelFormat::KA8 => (
                    data[index],
                    data[index],
                    data[index],
                    data.get(index + 1).copied().unwrap_or(255),
                ),
            };
            rgba.extend_from_slice(&[red, green, blue, alpha]);
        }
    }
    (image.width, image.height, rgba)
}

fn blit_glyph(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    pen: f32,
    baseline: f32,
    glyph: &Glyph,
    color: u32,
) {
    let left = (pen + glyph.x_offset).round() as i32 + glyph.xmin;
    let top = glyph_top(baseline, glyph);
    for row in 0..glyph.height {
        for col in 0..glyph.width {
            let alpha = glyph.coverage[row * glyph.width + col];
            if alpha == 0 {
                continue;
            }
            blend(
                pixels,
                width,
                height,
                left + col as i32,
                top + row as i32,
                color,
                alpha,
            );
        }
    }
}

fn fill_rect(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: u32,
) {
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

fn stroke_rect(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: u32,
) {
    fill_rect(pixels, width, height, x, y, w, 1.0, color);
    fill_rect(pixels, width, height, x, y + h - 1.0, w, 1.0, color);
    fill_rect(pixels, width, height, x, y, 1.0, h, color);
    fill_rect(pixels, width, height, x + w - 1.0, y, 1.0, h, color);
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
    fn close_button_is_the_window_corner() {
        assert_eq!(caption_hit(1280.0, 1260.0), Some(FrameHit::Close));
        assert_eq!(caption_hit(1280.0, 1200.0), Some(FrameHit::Maximize));
        assert_eq!(caption_hit(1280.0, 1160.0), Some(FrameHit::Minimize));
        assert_eq!(caption_hit(1280.0, 1100.0), None);
    }

    #[test]
    fn location_parser_uses_https_and_not_a_google_host() {
        let url = parse_location("example.com");
        assert_eq!(url.as_str(), "https://example.com/");
        let search = parse_location("mithu browser");
        assert_eq!(search.domain(), Some("duckduckgo.com"));
        assert_eq!(
            overlay_for(&Url::parse("about:settings").unwrap()),
            Some(Overlay::Settings)
        );
    }
}
