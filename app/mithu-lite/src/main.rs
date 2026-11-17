//! Mithu Lite window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//!
//! The frame is [`mithu_ui`], the same painter the Servo build uses. Each tab
//! is a child [`wry::WebView`] on the OS engine (WebKitGTK on Linux, WebView2
//! on Windows, WKWebView on macOS). This package does not link Servo.
//!
//! wry 0.57 exposes back, forward, and reload. It has no stop method; Stop
//! calls `window.stop()` in the page.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mithu_text::FontBook;
use mithu_ui::chrome::{self, Hit, Popup, Secure, TabPaint};
use mithu_ui::parse_location;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};
use wry::dpi::{PhysicalPosition as WryPos, PhysicalSize as WrySize};
use wry::{PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder};


fn app_icon() -> Option<winit::window::Icon> {
    let image = mithu_ui::brand::window_icon(true);
    winit::window::Icon::from_rgba(image.rgba, image.width, image.height).ok()
}

fn main() {
    mithu_host::init("mithu-lite");
    let config = Config::from_args();
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    init_gtk();
    let event_loop = match EventLoop::new() {
        Ok(loop_) => loop_,
        Err(error) => {
            mithu_host::report_fatal(&format!("could not create the event loop: {error}"))
        }
    };
    let mut app = App::new(config);
    if let Err(error) = event_loop.run_app(&mut app) {
        mithu_host::report_fatal(&format!("could not run the window: {error}"));
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
fn init_gtk() {
    use gtk::prelude::DisplayExtManual;
    if gtk::init().is_err() {
        eprintln!("mithu-lite: GTK failed to start");
        std::process::exit(1);
    }
    if gtk::gdk::Display::default().is_some_and(|display| display.backend().is_wayland()) {
        eprintln!("mithu-lite: the frame shares an X11 window with WebKit. Set GDK_BACKEND=x11.");
        std::process::exit(1);
    }
}

fn pump_host() {
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

struct Config {
    url: Option<String>,
    screenshot: Option<PathBuf>,
    hold: Option<Duration>,
    width: f32,
    height: f32,
}

impl Config {
    fn from_args() -> Self {
        let mut args = std::env::args().skip(1);
        let mut screenshot = None;
        let mut hold = None;
        let mut url = None;
        let mut sized = false;
        while let Some(arg) = args.next() {
            if arg == "--version" || arg == "-V" {
                println!("mithu-lite {}", env!("MITHU_VERSION"));
                std::process::exit(0);
            }
            if arg == "--screenshot" {
                screenshot = Some(PathBuf::from(args.next().unwrap_or_else(|| "mithu-lite.png".into())));
                sized = true;
                url = args.next();
                break;
            }
            if arg == "--hold" {
                let seconds = args.next().and_then(|value| value.parse().ok()).unwrap_or(8);
                hold = Some(Duration::from_secs(seconds));
                sized = true;
                url = args.next();
                break;
            }
            url = Some(arg);
            break;
        }
        Self {
            url,
            screenshot,
            hold,
            width: if sized { 1280.0 } else { 1200.0 },
            height: 800.0,
        }
    }
}

struct TabInfo {
    id: u64,
    title: String,
    url: String,
    loading: bool,
    can_back: bool,
    can_forward: bool,
}

struct Model {
    tabs: Vec<TabInfo>,
    active: usize,
    omnibox: String,
    omnibox_focused: bool,
    cursor: usize,
    select_all: bool,
    bookmarks: Vec<(String, String)>,
    bookmarks_open: bool,
    visits: Vec<String>,
    downloads: Vec<String>,
    pointer: (f32, f32),
    pressed: bool,
    link_status: String,
    menu: Option<Popup>,
    menu_name: String,
    submenu: Option<mithu_ui::menus::MenuId>,
    quit: bool,
    tab_search: bool,
    extensions: bool,
    profile: bool,
    suggestion: Option<usize>,
    find_open: bool,
    find_query: String,
    find_focused: bool,
    ntp: bool,
    next_id: u64,
    pending_tabs: Vec<String>,
    load_started: Option<Instant>,
    load_ms: Option<u128>,
    settled: bool,
    navigate: Option<String>,
    prefs: mithu_ui::pages::Prefs,
}

impl Model {
    fn new() -> Self {
        let prefs = mithu_ui::pages::load(&mithu_ui::pages::path());
        mithu_ui::set_search_engine(&prefs.search);
        let bookmarks = prefs.bookmarks.iter().map(|mark| (mark.title.clone(), mark.url.clone())).collect();
        let bookmarks_open = prefs.show_bookmarks;
        Self {
            tabs: Vec::new(),
            active: 0,
            omnibox: "about:newtab".into(),
            omnibox_focused: false,
            cursor: 0,
            select_all: false,
            bookmarks,
            bookmarks_open,
            visits: Vec::new(),
            downloads: Vec::new(),
            pointer: (-1.0, -1.0),
            pressed: false,
            link_status: String::new(),
            menu: None,
            menu_name: "app".into(),
            submenu: None,
            quit: false,
            tab_search: false,
            extensions: false,
            profile: false,
            suggestion: None,
            find_open: false,
            find_query: String::new(),
            find_focused: false,
            ntp: true,
            next_id: 1,
            pending_tabs: Vec::new(),
            load_started: None,
            load_ms: None,
            settled: false,
            navigate: None,
            prefs,
        }
    }

    fn active(&self) -> Option<&TabInfo> {
        self.tabs.get(self.active)
    }

    fn active_mut(&mut self) -> Option<&mut TabInfo> {
        self.tabs.get_mut(self.active)
    }

    fn tab_mut(&mut self, id: u64) -> Option<&mut TabInfo> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }

    fn suggestions(&self) -> Vec<String> {
        if !self.omnibox_focused {
            return Vec::new();
        }
        let needle = self.omnibox.to_ascii_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (title, url) in &self.bookmarks {
            if title.to_ascii_lowercase().contains(&needle) || url.to_ascii_lowercase().contains(&needle) {
                out.push(url.clone());
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
        out.truncate(6);
        out
    }
}

struct Tab {
    id: u64,
    webview: WebView,
}

struct App {
    config: Config,
    shared: Arc<Mutex<Model>>,
    window: Option<Arc<Window>>,
    surface_context: Option<softbuffer::Context<Arc<Window>>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    web_context: WebContext,
    tabs: Vec<Tab>,
    fonts: FontBook,
    modifiers: ModifiersState,
    started: Instant,
    reported: bool,
    reported_at: Option<Instant>,
    captured: bool,
    rss_mark: Instant,
    scale: f32,
}

impl App {
    fn new(config: Config) -> Self {
        let mut model = Model::new();
        if config.url.is_some() {
            model.ntp = false;
            model.settled = false;
        }
        Self {
            config,
            shared: Arc::new(Mutex::new(model)),
            window: None,
            surface_context: None,
            surface: None,
            web_context: WebContext::new(Some(profile_dir())),
            tabs: Vec::new(),
            fonts: FontBook::load(),
            modifiers: ModifiersState::default(),
            started: Instant::now(),
            reported: false,
            reported_at: None,
            captured: false,
            rss_mark: Instant::now(),
            scale: 1.0,
        }
    }

    fn model(&self) -> std::sync::MutexGuard<'_, Model> {
        self.shared.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn open_tab(&mut self, url: &str, show_ntp: bool) {
        let id = {
            let mut model = self.model();
            let id = model.next_id;
            model.next_id += 1;
            model.tabs.push(TabInfo {
                id,
                title: "New Tab".into(),
                url: if show_ntp { "about:newtab".into() } else { url.to_string() },
                loading: !show_ntp && !url.is_empty(),
                can_back: false,
                can_forward: false,
            });
            model.active = model.tabs.len() - 1;
            model.ntp = show_ntp;
            model.omnibox = if show_ntp { "about:newtab".into() } else { url.to_string() };
            model.omnibox_focused = false;
            model.menu = None;
            model.tab_search = false;
            id
        };
        let Some(window) = self.window.clone() else {
            return;
        };
        let webview = match self.build_webview(&window, id) {
            Ok(webview) => webview,
            Err(error) => {
                eprintln!("mithu-lite: could not open a webview: {error}");
                return;
            }
        };
        self.tabs.push(Tab { id, webview });
        self.apply_bounds();
        if show_ntp {
            self.load_in_active("mithu://newtab");
        } else if !url.is_empty() {
            self.load_in_active(url);
        }
        self.request_redraw();
    }

    fn build_webview(&mut self, window: &Window, id: u64) -> wry::Result<WebView> {
        let shared = self.shared.clone();
        let bounds = self.content_rect();
        WebViewBuilder::new_with_web_context(&mut self.web_context)
            .with_bounds(bounds)
            .with_visible(false)
            .with_focused(false)
            .with_initialization_script(INIT_SCRIPT)
            .with_ipc_handler({
                let shared = shared.clone();
                move |request| on_ipc(&shared, id, request.body())
            })
            .with_document_title_changed_handler({
                let shared = shared.clone();
                move |title| {
                    let mut model = lock(&shared);
                    if let Some(tab) = model.tab_mut(id) {
                        if !title.trim().is_empty() {
                            tab.title = title;
                        }
                    }
                }
            })
            .with_on_page_load_handler({
                let shared = shared.clone();
                move |event, url| on_load(&shared, id, event, &url)
            })
            .with_navigation_handler({
                let shared = shared.clone();
                move |url| {
                    let mut model = lock(&shared);
                    if url.starts_with("mithu:") || (url.starts_with("about:") && url != "about:blank") {
                        model.navigate = Some(url);
                        return false;
                    }
                    if let Some(tab) = model.tab_mut(id) {
                        if url.starts_with("http://") || url.starts_with("https://") {
                            tab.url = url;
                            tab.loading = true;
                        }
                    }
                    true
                }
            })
            .with_new_window_req_handler({
                let shared = shared.clone();
                move |url, _features| {
                    lock(&shared).pending_tabs.push(url);
                    wry::NewWindowResponse::Deny
                }
            })
            .with_download_started_handler(|url, path| {
                let name = suggested_name(&url, path);
                let prefs = mithu_ui::pages::load(&mithu_ui::pages::path());
                if prefs.download_dir.is_empty() {
                    *path = unique_download(&name);
                } else {
                    let dir = PathBuf::from(&prefs.download_dir);
                    let _ = std::fs::create_dir_all(&dir);
                    *path = dir.join(name);
                }
                true
            })
            .with_download_completed_handler({
                let shared = shared.clone();
                move |_url, path, success| {
                    if success {
                        if let Some(path) = path {
                            let shown = path.display().to_string();
                            let mut model = lock(&shared);
                            model.downloads.push(shown.clone());
                            model.prefs.note_download(&_url, &shown, "done");
                            mithu_ui::pages::save(&mithu_ui::pages::path(), &model.prefs);
                        }
                    }
                }
            })
            .build_as_child(window)
    }

    fn content_rect(&self) -> Rect {
        let model = self.model();
        let top = chrome_top(model.bookmarks_open, self.scale);
        Rect {
            position: WryPos::new(0, top as i32).into(),
            size: WrySize::new(
                self.config.width.max(1.0) as u32,
                (self.config.height - top).max(1.0) as u32,
            )
            .into(),
        }
    }

    fn apply_bounds(&mut self) {
        let (active, ntp, top, width, height) = {
            let model = self.model();
            let active = model.active().map(|tab| tab.id);
            (active, model.ntp, chrome_top(model.bookmarks_open, self.scale), self.config.width, self.config.height)
        };
        let bounds = Rect {
            position: WryPos::new(0, top as i32).into(),
            size: WrySize::new(width.max(1.0) as u32, (height - top).max(1.0) as u32).into(),
        };
        for tab in &self.tabs {
            let show = Some(tab.id) == active && !ntp;
            let _ = tab.webview.set_visible(show);
            if show {
                let _ = tab.webview.set_bounds(bounds);
            }
        }
    }

    fn active_webview(&self) -> Option<&WebView> {
        let id = self.model().active().map(|tab| tab.id)?;
        self.tabs.iter().find(|tab| tab.id == id).map(|tab| &tab.webview)
    }

    fn load_in_active(&mut self, input: &str) {
        let trimmed = input.trim();
        if trimmed.starts_with("mithu://do") {
            self.run_page_command(trimmed);
            return;
        }
        let prefs = self.model().prefs.clone();
        if let Some(document) = mithu_ui::pages::open(trimmed, &prefs, env!("MITHU_VERSION")) {
            {
                let mut model = self.model();
                model.ntp = false;
                model.omnibox = document.url.clone();
                model.omnibox_focused = false;
                model.settled = true;
                model.load_ms = Some(0);
                if let Some(tab) = model.active_mut() {
                    tab.url = document.url.clone();
                    tab.title = document.title.clone();
                    tab.loading = false;
                }
            }
            self.apply_bounds();
            if let Some(webview) = self.active_webview() {
                let _ = webview.load_html(&document.html);
            }
            self.request_redraw();
            return;
        }
        let url = parse_location(input);
        let spec = url.to_string();
        {
            let mut model = self.model();
            model.ntp = false;
            model.omnibox = spec.clone();
            model.omnibox_focused = false;
            model.settled = false;
            model.load_ms = None;
            model.load_started = Some(Instant::now());
            model.link_status.clear();
            if let Some(tab) = model.active_mut() {
                tab.url = spec.clone();
                tab.loading = true;
            }
        }
        self.apply_bounds();
        if let Some(webview) = self.active_webview() {
            if let Err(error) = webview.load_url(&spec) {
                eprintln!("mithu-lite: navigation failed: {error}");
            }
            let _ = webview.focus();
        }
        self.request_redraw();
    }

    fn run_page_command(&mut self, spec: &str) {
        let mut prefs = self.model().prefs.clone();
        let Some(command) = mithu_ui::pages::command(spec, &mut prefs) else {
            return;
        };
        mithu_ui::pages::save(&mithu_ui::pages::path(), &prefs);
        mithu_ui::set_search_engine(&prefs.search);
        {
            let mut model = self.model();
            model.prefs = prefs.clone();
            model.bookmarks_open = prefs.show_bookmarks;
            model.bookmarks = prefs.bookmarks.iter().map(|mark| (mark.title.clone(), mark.url.clone())).collect();
        }
        if let Some(path) = command.open_path.as_deref() {
            reveal_path(path, false);
        }
        if let Some(path) = command.reveal_path.as_deref() {
            reveal_path(path, true);
        }
        if let Some(webview) = self.active_webview() {
            let _ = webview.zoom(prefs.zoom as f64);
        }
        self.load_in_active(&command.redirect);
    }

    fn sync_history_flags(&self) {
        let Some(webview) = self.active_webview() else {
            return;
        };
        let back = webview.can_go_back().unwrap_or(false);
        let forward = webview.can_go_forward().unwrap_or(false);
        let mut model = self.model();
        if let Some(tab) = model.active_mut() {
            tab.can_back = back;
            tab.can_forward = forward;
        }
    }

    fn go_back(&mut self) {
        if self.model().ntp {
            return;
        }
        if let Some(webview) = self.active_webview() {
            let _ = webview.go_back();
        }
    }

    fn go_forward(&mut self) {
        if let Some(webview) = self.active_webview() {
            let _ = webview.go_forward();
        }
    }

    fn reload_or_stop(&mut self) {
        let loading = self.model().active().is_some_and(|tab| tab.loading);
        let Some(webview) = self.active_webview() else {
            return;
        };
        if loading {
            // wry has go_back, go_forward, and reload, and no stop method.
            let _ = webview.evaluate_script("window.stop()");
            self.model().active_mut().map(|tab| tab.loading = false);
        } else {
            let _ = webview.reload();
        }
    }

    fn click(&mut self, x: f32, y: f32) {
        let hit = {
            let model = self.model();
            chrome::hit(&self.paint_state(&model), x, y)
        };
        match hit {
            Hit::Drag => {
                if let Some(window) = &self.window {
                    let _ = window.drag_window();
                }
            }
            Hit::Minimize => {
                if let Some(window) = &self.window {
                    let _ = window.set_minimized(true);
                }
            }
            Hit::Maximize => {
                if let Some(window) = &self.window {
                    window.set_maximized(!window.is_maximized());
                }
            }
            Hit::Close => {
                if let Some(window) = &self.window {
                    let _ = window.set_visible(false);
                }
                std::process::exit(0);
            }
            Hit::Tab(index) => self.select_tab(index),
            Hit::CloseTab(index) => self.close_index(index),
            Hit::NewTab => self.open_tab("about:newtab", true),
            Hit::TabSearch => {
                let mut model = self.model();
                model.tab_search = !model.tab_search;
                model.menu = None;
                model.extensions = false;
                model.profile = false;
            }
            Hit::Back => self.go_back(),
            Hit::Forward => self.go_forward(),
            Hit::Reload => self.reload_or_stop(),
            Hit::Home => self.load_in_active("about:newtab"),
            Hit::Omnibox => self.focus_omnibox(),
            Hit::Star => self.toggle_bookmark(),
            Hit::Extensions => {
                let mut model = self.model();
                model.extensions = !model.extensions;
                model.menu = None;
                model.tab_search = false;
                model.profile = false;
            }
            Hit::Profile => {
                let mut model = self.model();
                model.profile = !model.profile;
                model.menu = None;
                model.tab_search = false;
                model.extensions = false;
            }
            Hit::Menu => {
                let mut model = self.model();
                model.submenu = None;
                model.menu_name = "app".into();
                model.menu = if model.menu == Some(Popup::App) { None } else { Some(Popup::App) };
                model.tab_search = false;
                model.extensions = false;
                model.profile = false;
            }
            Hit::Bookmark(index) => {
                let url = self.model().bookmarks.get(index).map(|mark| mark.1.clone());
                if let Some(url) = url {
                    self.load_in_active(&url);
                }
            }
            Hit::MenuItem(index) => self.run_menu(index, false),
            Hit::SubmenuItem(index) => self.run_menu(index, true),
            Hit::Zoom(kind) => {
                self.model().menu = None;
                self.model().submenu = None;
                let next = match kind {
                    -1 => (self.model().prefs.zoom / 1.1).clamp(0.5, 3.0),
                    1 => (self.model().prefs.zoom * 1.1).clamp(0.5, 3.0),
                    2 => {
                        if let Some(window) = &self.window {
                            window.set_maximized(!window.is_maximized());
                        }
                        self.model().prefs.zoom
                    }
                    _ => 1.0,
                };
                self.model().prefs.zoom = next;
                if let Some(webview) = self.active_webview() {
                    let _ = webview.zoom(next as f64);
                }
            }
            Hit::Suggestion(index) => {
                let url = self.model().suggestions().get(index).cloned();
                if let Some(url) = url {
                    self.load_in_active(&url);
                }
            }
            Hit::Content => {
                self.model().omnibox_focused = false;
                self.model().menu = None;
                if let Some(webview) = self.active_webview() {
                    let _ = webview.focus();
                }
            }
            Hit::Panel | Hit::None => {}
        }
        self.request_redraw();
    }

    fn select_tab(&mut self, index: usize) {
        {
            let mut model = self.model();
            if index >= model.tabs.len() {
                return;
            }
            model.active = index;
            model.tab_search = false;
            model.menu = None;
            let ntp = model.tabs[index].url.starts_with("about:newtab");
            model.ntp = ntp;
            model.omnibox = model.tabs[index].url.clone();
        }
        self.apply_bounds();
    }

    fn close_index(&mut self, index: usize) {
        let id = {
            let mut model = self.model();
            if model.tabs.len() <= 1 {
                model.navigate = Some("about:newtab".into());
                return;
            }
            let id = model.tabs[index].id;
            let url = model.tabs[index].url.clone();
            if url.starts_with("http") {
                model.visits.push(url);
            }
            model.tabs.remove(index);
            if model.active >= model.tabs.len() {
                model.active = model.tabs.len() - 1;
            } else if index < model.active {
                model.active -= 1;
            }
            let ntp = model.active().is_some_and(|tab| tab.url.starts_with("about:newtab"));
            model.ntp = ntp;
            if let Some(tab) = model.active() {
                model.omnibox = tab.url.clone();
            }
            id
        };
        self.tabs.retain(|tab| tab.id != id);
        self.apply_bounds();
    }

    fn run_menu(&mut self, index: usize, from_submenu: bool) {
        use mithu_ui::menus::MenuId;
        let id = {
            let model = self.model();
            let rows = if from_submenu {
                model.submenu.map(mithu_ui::menus::children).unwrap_or_default()
            } else {
                mithu_ui::menus::rows_named(&model.menu_name)
            };
            rows.get(index).map(|row| row.id)
        };
        let Some(id) = id else {
            return;
        };
        if !from_submenu && !mithu_ui::menus::children(id).is_empty() {
            self.model().submenu = Some(id);
            return;
        }
        {
            let mut model = self.model();
            model.menu = None;
            model.submenu = None;
        }
        match id {
            MenuId::NewTab | MenuId::NewWindow | MenuId::OpenLinkTab | MenuId::OpenLinkWindow | MenuId::OpenImageTab => {
                self.open_tab("mithu://newtab", false);
            }
            MenuId::History => self.load_in_active("mithu://history"),
            MenuId::Downloads => self.load_in_active("mithu://downloads"),
            MenuId::Bookmarks | MenuId::BookmarkManager => self.load_in_active("mithu://bookmarks"),
            MenuId::Settings | MenuId::Help => self.load_in_active("mithu://settings"),
            MenuId::About => self.load_in_active("mithu://version"),
            MenuId::Passwords | MenuId::PasswordManager | MenuId::Payments | MenuId::Addresses => {
                self.load_in_active("mithu://settings/autofill");
            }
            MenuId::Extensions | MenuId::ManageExtensions => self.load_in_active("mithu://extensions"),
            MenuId::DeleteData => self.load_in_active("mithu://settings/privacy"),
            MenuId::Performance => self.load_in_active("mithu://settings/performance"),
            MenuId::Bookmark => self.toggle_bookmark(),
            MenuId::ShowBookmarksBar => {
                let open = !self.model().bookmarks_open;
                self.model().bookmarks_open = open;
            }
            MenuId::Back => self.go_back(),
            MenuId::Forward => self.go_forward(),
            MenuId::Reload => self.reload_or_stop(),
            MenuId::Find | MenuId::FindNext => {
                self.model().find_open = true;
            }
            MenuId::Exit => self.model().quit = true,
            MenuId::CloseTab => {
                let index = self.model().active;
                self.close_index(index);
            }
            _ => {}
        }
    }

    fn focus_omnibox(&mut self) {
        {
            let mut model = self.model();
            if let Some(tab) = model.active() {
                model.omnibox = tab.url.clone();
            }
            model.cursor = model.omnibox.chars().count();
            model.select_all = true;
            model.omnibox_focused = true;
            model.find_focused = false;
            model.suggestion = None;
        }
        if let Some(webview) = self.active_webview() {
            let _ = webview.focus_parent();
        }
    }

    fn toggle_bookmark(&mut self) {
        let mut model = self.model();
        let Some(url) = model.active().map(|tab| tab.url.clone()) else {
            return;
        };
        if !url.starts_with("http") {
            return;
        }
        if let Some(index) = model.bookmarks.iter().position(|mark| mark.1 == url) {
            model.bookmarks.remove(index);
        } else {
            let title = model
                .active()
                .map(|tab| tab.title.clone())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| url.clone());
            model.bookmarks.push((title, url));
        }
    }

    fn insert_char(&mut self, ch: char) {
        let mut model = self.model();
        if !model.omnibox_focused && !model.find_focused {
            return;
        }
        if model.find_focused {
            model.find_query.push(ch);
            return;
        }
        if model.select_all {
            model.omnibox.clear();
            model.cursor = 0;
            model.select_all = false;
        }
        let byte = byte_at(&model.omnibox, model.cursor);
        model.omnibox.insert(byte, ch);
        model.cursor += 1;
        model.suggestion = None;
    }

    fn backspace(&mut self) {
        let mut model = self.model();
        if model.find_focused {
            model.find_query.pop();
            return;
        }
        if !model.omnibox_focused {
            return;
        }
        if model.select_all {
            model.omnibox.clear();
            model.cursor = 0;
            model.select_all = false;
            return;
        }
        if model.cursor == 0 {
            return;
        }
        model.cursor -= 1;
        let byte = byte_at(&model.omnibox, model.cursor);
        model.omnibox.remove(byte);
    }

    fn submit(&mut self) {
        let (find, query, text) = {
            let model = self.model();
            if model.find_focused {
                (true, model.find_query.clone(), String::new())
            } else if let Some(index) = model.suggestion {
                let url = model.suggestions().get(index).cloned().unwrap_or_else(|| model.omnibox.clone());
                (false, String::new(), url)
            } else {
                (false, String::new(), model.omnibox.clone())
            }
        };
        if find {
            if let Some(webview) = self.active_webview() {
                let script = format!("window.find({})", js_string(&query));
                let _ = webview.evaluate_script(&script);
            }
            return;
        }
        self.load_in_active(&text);
    }

    fn shortcut(&mut self, key: &str) {
        match key {
            "new" => self.open_tab("about:newtab", true),
            "close" => {
                let index = self.model().active;
                self.close_index(index);
            }
            "location" => self.focus_omnibox(),
            "reload" => self.reload_or_stop(),
            "back" => self.go_back(),
            "forward" => self.go_forward(),
            "find" => {
                let mut model = self.model();
                model.find_open = true;
                model.find_focused = true;
                model.omnibox_focused = false;
            }
            "bookmarks" => {
                let mut model = self.model();
                model.bookmarks_open = !model.bookmarks_open;
            }
            "history" => self.load_in_active("about:history"),
            "downloads" => self.load_in_active("about:downloads"),
            "settings" => self.load_in_active("about:settings"),
            "next" => {
                let index = {
                    let model = self.model();
                    if model.tabs.is_empty() {
                        return;
                    }
                    (model.active + 1) % model.tabs.len()
                };
                self.select_tab(index);
            }
            "prev" => {
                let index = {
                    let model = self.model();
                    if model.tabs.is_empty() {
                        return;
                    }
                    if model.active == 0 { model.tabs.len() - 1 } else { model.active - 1 }
                };
                self.select_tab(index);
            }
            _ => {}
        }
    }

    fn paint_state<'a>(&'a self, model: &'a Model) -> chrome::State<'a> {
        let tabs = model
            .tabs
            .iter()
            .map(|tab| TabPaint {
                title: if tab.title.is_empty() { "New Tab".into() } else { tab.title.clone() },
                favicon: None,
            })
            .collect();
        let location = model.active().map(|tab| tab.url.as_str()).unwrap_or("");
        let secure = if location.starts_with("https://") {
            Secure::Https
        } else if location.starts_with("http://") {
            Secure::Http
        } else {
            Secure::Internal
        };
        chrome::State {
            width: self.config.width,
            height: self.config.height,
            tabs,
            active: model.active,
            omnibox: model.omnibox.clone(),
            omnibox_focused: model.omnibox_focused,
            cursor: model.cursor,
            secure,
            bookmarked: model.bookmarks.iter().any(|mark| same_url(&mark.1, location)),
            bookmarks: model.bookmarks.clone(),
            bookmarks_open: model.bookmarks_open,
            can_back: model.active().is_some_and(|tab| tab.can_back),
            can_forward: model.active().is_some_and(|tab| tab.can_forward),
            loading: model.active().is_some_and(|tab| tab.loading) && !model.ntp,
            progress: if model.active().is_some_and(|tab| tab.loading) { 0.45 } else { 0.0 },
            link_status: model.link_status.clone(),
            pointer: model.pointer,
            pressed: model.pressed,
            ntp: model.ntp,
            menu: model.menu,
            menu_labels: menu_labels(model),
            submenu_at: model.submenu.and_then(|id| {
                mithu_ui::menus::rows_named(&model.menu_name).iter().position(|row| row.id == id)
            }),
            submenu_labels: model
                .submenu
                .map(|id| mithu_ui::menus::labels(&mithu_ui::menus::children(id), (model.prefs.zoom * 100.0).round() as u32))
                .unwrap_or_default(),
            tab_search: model.tab_search,
            extensions: model.extensions,
            profile: model.profile,
            suggestions: model.suggestions(),
            suggestion: model.suggestion,
            find_open: model.find_open,
            find_query: model.find_query.clone(),
            fonts: &self.fonts,
            scale: self.scale,
            dark: model.prefs.dark(),
            show_home: model.prefs.show_home,
        }
    }

    fn redraw(&mut self) {
        self.sync_history_flags();
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        self.config.width = width as f32;
        self.config.height = height as f32;
        self.scale = window.scale_factor() as f32;
        let Some(width) = std::num::NonZeroU32::new(width) else {
            return;
        };
        let Some(height) = std::num::NonZeroU32::new(height) else {
            return;
        };
        if surface.resize(width, height).is_err() {
            return;
        }
        let px_w = width.get();
        let px_h = height.get();
        let pixels = {
            let mut pixels = vec![0x00ff_ffff; (px_w as usize) * (px_h as usize)];
            let model = self.model();
            let state = self.paint_state(&model);
            chrome::paint(&mut pixels, px_w, px_h, &state);
            pixels
        };
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        let count = buffer.len().min(pixels.len());
        buffer[..count].copy_from_slice(&pixels[..count]);
        let _ = buffer.present();
        self.apply_bounds();
        if let Some(window) = &self.window {
            let title = self.model().active().map(|tab| tab.title.clone()).unwrap_or_default();
            if title.is_empty() || title == "New Tab" {
                window.set_title(&format!("Mithu Lite {}", env!("MITHU_VERSION")));
            } else {
                window.set_title(&format!("{title} — Mithu Lite {}", env!("MITHU_VERSION")));
            }
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn poll_shared(&mut self) {
        let (pending, action) = {
            let mut model = self.model();
            let pending = std::mem::take(&mut model.pending_tabs);
            let action = model.navigate.take();
            (pending, action)
        };
        for url in pending {
            self.open_tab(&url, false);
        }
        if let Some(action) = action {
            if let Some(key) = action.strip_prefix("key:") {
                let key = key.to_string();
                self.shortcut(&key);
            } else {
                self.load_in_active(&action);
            }
        }
    }

    fn maybe_finish(&mut self, event_loop: &ActiveEventLoop) {
        let measuring = self.config.screenshot.is_some() || self.config.hold.is_some();
        if !measuring {
            return;
        }
        let (settled, load_ms) = {
            let model = self.model();
            (model.settled, model.load_ms)
        };
        let timed_out = self.started.elapsed() > Duration::from_secs(45);
        if !self.reported && (settled || timed_out) {
            match load_ms {
                Some(ms) => println!("load_ms {ms}"),
                None if timed_out => println!("load_ms timeout"),
                None => return,
            }
            println!("rss_kb {}", rss_kb());
            self.reported = true;
            self.reported_at = Some(Instant::now());
            self.rss_mark = Instant::now();
        }
        if !self.reported {
            return;
        }
        if let Some(path) = self.config.screenshot.clone() {
            if self.captured {
                return;
            }
            let quiet = self.reported_at.is_some_and(|start| start.elapsed() > Duration::from_millis(900));
            if !quiet && !timed_out {
                return;
            }
            self.captured = true;
            if capture_root(&path) {
                println!("screenshot {}", path.display());
            } else {
                eprintln!("mithu-lite: root capture failed");
            }
            event_loop.exit();
            return;
        }
        if let Some(hold) = self.config.hold {
            if self.rss_mark.elapsed() >= Duration::from_secs(1) {
                println!("rss_kb {}", rss_kb());
                self.rss_mark = Instant::now();
            }
            let dwell = self.reported_at.map(|start| start.elapsed()).unwrap_or_default();
            if dwell >= hold {
                println!("rss_kb {}", rss_kb());
                event_loop.exit();
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(format!("Mithu Lite {}", env!("MITHU_VERSION")))
            .with_window_icon(app_icon())
            .with_decorations(false)
            .with_inner_size(winit::dpi::LogicalSize::new(self.config.width, self.config.height))
            .with_position(PhysicalPosition::new(0, 0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("mithu-lite: could not open a window: {error}");
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("mithu-lite: could not create a framebuffer: {error}");
                event_loop.exit();
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => {
                eprintln!("mithu-lite: could not create a surface: {error}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        self.config.width = size.width.max(1) as f32;
        self.config.height = size.height.max(1) as f32;
        self.surface_context = Some(context);
        self.surface = Some(surface);
        self.window = Some(window);
        self.started = Instant::now();
        if let Some(url) = self.config.url.clone() {
            self.open_tab(&url, false);
        } else {
            let startup = {
                let prefs = self.model().prefs.clone();
                match prefs.startup.as_str() {
                    "continue" => prefs.history.last().map(|visit| visit.url.clone()),
                    "pages" => prefs.startup_pages.first().cloned(),
                    _ => None,
                }
            };
            if let Some(url) = startup {
                self.open_tab(&url, false);
            } else {
                self.open_tab("about:newtab", true);
                let mut model = self.model();
                model.settled = true;
                model.load_ms = Some(0);
            }
        }
        self.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.config.width = size.width.max(1) as f32;
                self.config.height = size.height.max(1) as f32;
                self.apply_bounds();
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => {
                self.model().pointer = (position.x as f32, position.y as f32);
                self.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (x, y) = self.model().pointer;
                if button == MouseButton::Right && state == ElementState::Pressed {
                    let scale = if self.scale >= 1.0 { self.scale } else { 1.0 };
                    let tab_h = (41.0 * scale).round();
                    let tool_h = (48.0 * scale).round();
                    let name = if y < tab_h {
                        "tab"
                    } else if y < tab_h + tool_h {
                        "omnibox"
                    } else {
                        "page"
                    };
                    let mut model = self.model();
                    model.menu_name = name.into();
                    model.menu = Some(Popup::Context);
                    model.submenu = None;
                } else if button == MouseButton::Left && state == ElementState::Pressed {
                    self.model().pressed = true;
                    self.click(x, y);
                    if self.model().quit {
                        event_loop.exit();
                    }
                } else if button == MouseButton::Left && state == ElementState::Released {
                    self.model().pressed = false;
                    self.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                let ctrl = self.modifiers.control_key();
                let alt = self.modifiers.alt_key();
                let shift = self.modifiers.shift_key();
                let focused = self.model().omnibox_focused;
                match event.logical_key {
                    Key::Named(NamedKey::Enter) if focused || self.model().find_focused => self.submit(),
                    Key::Named(NamedKey::Escape) => {
                        let mut model = self.model();
                        model.menu = None;
                        model.omnibox_focused = false;
                        model.find_open = false;
                        model.find_focused = false;
                        model.tab_search = false;
                    }
                    Key::Named(NamedKey::Backspace) if focused || self.model().find_focused => self.backspace(),
                    Key::Named(NamedKey::ArrowLeft) if alt => self.go_back(),
                    Key::Named(NamedKey::ArrowRight) if alt => self.go_forward(),
                    Key::Named(NamedKey::ArrowLeft) if focused => {
                        let mut model = self.model();
                        model.select_all = false;
                        model.cursor = model.cursor.saturating_sub(1);
                    }
                    Key::Named(NamedKey::ArrowRight) if focused => {
                        let mut model = self.model();
                        model.select_all = false;
                        let len = model.omnibox.chars().count();
                        model.cursor = (model.cursor + 1).min(len);
                    }
                    Key::Named(NamedKey::ArrowDown) if focused => {
                        let mut model = self.model();
                        let count = model.suggestions().len();
                        if count > 0 {
                            let next = model.suggestion.map(|i| (i + 1).min(count - 1)).unwrap_or(0);
                            model.suggestion = Some(next);
                        }
                    }
                    Key::Named(NamedKey::ArrowUp) if focused => {
                        let mut model = self.model();
                        if let Some(current) = model.suggestion {
                            model.suggestion = Some(current.saturating_sub(1));
                        }
                    }
                    Key::Named(NamedKey::F5) => self.reload_or_stop(),
                    Key::Named(NamedKey::Tab) if ctrl && shift => self.shortcut("prev"),
                    Key::Named(NamedKey::Tab) if ctrl => self.shortcut("next"),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("t") => self.shortcut("new"),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("w") => self.shortcut("close"),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("l") => self.shortcut("location"),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("r") => self.shortcut("reload"),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("f") => self.shortcut("find"),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("b") => self.shortcut("bookmarks"),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("h") => self.shortcut("history"),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("j") => self.shortcut("downloads"),
                    Key::Character(text) if ctrl && text == "," => self.shortcut("settings"),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("a") && focused => {
                        self.model().select_all = true;
                    }
                    Key::Character(text) if (focused || self.model().find_focused) && !ctrl && !alt => {
                        for ch in text.chars() {
                            self.insert_char(ch);
                        }
                    }
                    Key::Named(NamedKey::Space) if focused => self.insert_char(' '),
                    _ => {
                        let _ = shift;
                    }
                }
                self.request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        pump_host();
        self.poll_shared();
        self.maybe_finish(event_loop);
        if self.model().active().is_some_and(|tab| tab.loading) || !self.reported && self.config.hold.is_some() || self.config.screenshot.is_some()
        {
            self.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16)));
    }
}

fn on_load(shared: &Mutex<Model>, id: u64, event: PageLoadEvent, url: &str) {
    let mut model = lock(shared);
    let is_active = model.active().is_some_and(|tab| tab.id == id);
    let focused = model.omnibox_focused;
    let track = url.starts_with("http://") || url.starts_with("https://") || url == "about:blank";
    {
        let Some(tab) = model.tab_mut(id) else {
            return;
        };
        match event {
            PageLoadEvent::Started => {
                if track {
                    tab.url = url.to_string();
                    tab.loading = true;
                }
            }
            PageLoadEvent::Finished => {
                if track {
                    tab.url = url.to_string();
                }
                tab.loading = false;
            }
        }
    }
    if !track {
        return;
    }
    if is_active && !focused {
        model.omnibox = url.to_string();
        if !matches!(event, PageLoadEvent::Finished) {
            model.ntp = false;
        }
    }
    if matches!(event, PageLoadEvent::Started) && model.load_started.is_none() {
        model.load_started = Some(Instant::now());
    }
    if matches!(event, PageLoadEvent::Finished) {
        if url.starts_with("http") && !model.visits.iter().any(|visit| visit == url) {
            model.visits.push(url.to_string());
            let title = model.tab_mut(id).map(|tab| tab.title.clone()).unwrap_or_default();
            model.prefs.remember(url, &title);
            mithu_ui::pages::save(&mithu_ui::pages::path(), &model.prefs);
        }
        if is_active && model.load_ms.is_none() {
            let started = model.load_started.unwrap_or_else(Instant::now);
            model.load_ms = Some(started.elapsed().as_millis());
            model.settled = true;
        }
    }
}

fn on_ipc(shared: &Mutex<Model>, id: u64, body: &str) {
    let Some(kind) = json_field(body, "kind") else {
        return;
    };
    let mut model = lock(shared);
    if model.active().is_some_and(|tab| tab.id != id) && kind != "key" {
        return;
    }
    match kind.as_str() {
        "status" => {
            model.link_status = json_field(body, "status").unwrap_or_default();
        }
        "key" => {
            if let Some(key) = json_field(body, "key") {
                model.navigate = Some(format!("key:{key}"));
            }
        }
        _ => {}
    }
}

fn lock(shared: &Mutex<Model>) -> std::sync::MutexGuard<'_, Model> {
    shared.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn menu_labels(model: &Model) -> Vec<String> {
    if model.menu.is_none() {
        return Vec::new();
    }
    let zoom = (model.prefs.zoom * 100.0).round() as u32;
    mithu_ui::menus::labels(&mithu_ui::menus::rows_named(&model.menu_name), zoom)
}

fn reveal_path(path: &str, folder: bool) {
    let target = if folder {
        std::path::Path::new(path).parent().map(|dir| dir.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from(path))
    } else {
        std::path::PathBuf::from(path)
    };
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd").args(["/C", "start", "", &target.display().to_string()]).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&target).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&target).spawn();
    }
}

fn chrome_top(bookmarks: bool, scale: f32) -> f32 {
    chrome::chrome_height(bookmarks, scale)
}

fn byte_at(text: &str, cursor: usize) -> usize {
    text.char_indices().nth(cursor).map(|(index, _)| index).unwrap_or(text.len())
}

fn same_url(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}

fn json_field(body: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\":\"");
    let start = body.find(&pattern)? + pattern.len();
    let rest = &body[start..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else if ch == '"' {
            break;
        } else {
            out.push(ch);
        }
    }
    Some(out)
}

fn js_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn profile_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share")))
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("mithu-lite");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn downloads_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = home.join("Downloads");
    if std::fs::create_dir_all(&dir).is_ok() {
        dir
    } else {
        std::env::temp_dir()
    }
}

fn suggested_name(url: &str, path: &Path) -> String {
    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        if !name.is_empty() && name != "download" {
            return sanitize(name);
        }
    }
    let trimmed = url.split('?').next().unwrap_or(url);
    let name = trimmed.rsplit('/').next().filter(|name| !name.is_empty());
    sanitize(name.unwrap_or("download"))
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ' ') { ch } else { '_' })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() { "download".into() } else { cleaned.to_string() }
}

fn unique_download(name: &str) -> PathBuf {
    let dir = downloads_dir();
    let direct = dir.join(name);
    if !direct.exists() {
        return direct;
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("download");
    let ext = path.extension().and_then(|ext| ext.to_str());
    for index in 1..1000 {
        let candidate = match ext {
            Some(ext) => dir.join(format!("{stem}-{index}.{ext}")),
            None => dir.join(format!("{stem}-{index}")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(name)
}

fn rss_kb() -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.split_whitespace().next().unwrap_or("0").parse().unwrap_or(0);
        }
    }
    0
}

fn capture_root(path: &Path) -> bool {
    std::process::Command::new("import")
        .args(["-window", "root", &path.display().to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

const INIT_SCRIPT: &str = r#"(function () {
  function send(payload) {
    try { window.ipc.postMessage(JSON.stringify(payload)); } catch (e) {}
  }
  document.addEventListener("mouseover", function (event) {
    var node = event.target && event.target.closest ? event.target.closest("a[href]") : null;
    send({ kind: "status", status: node ? node.href : "" });
  }, true);
  document.addEventListener("mouseout", function (event) {
    var node = event.target && event.target.closest ? event.target.closest("a[href]") : null;
    if (node) send({ kind: "status", status: "" });
  }, true);
  window.addEventListener("keydown", function (event) {
    if (!event.ctrlKey && !event.metaKey) return;
    var key = event.key.toLowerCase();
    var name = null;
    if (key === "t" && !event.shiftKey) name = "new";
    else if (key === "w") name = "close";
    else if (key === "l") name = "location";
    else if (key === "r") name = "reload";
    else if (key === "f") name = "find";
    else if (key === "b") name = "bookmarks";
    else if (key === "h") name = "history";
    else if (key === "j") name = "downloads";
    else if (event.key === ",") name = "settings";
    else if (key === "tab" && event.shiftKey) name = "prev";
    else if (key === "tab") name = "next";
    if (!name) return;
    event.preventDefault();
    send({ kind: "key", key: name });
  }, true);
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_name_prefers_the_url_leaf() {
        assert_eq!(suggested_name("https://example.com/files/a b.txt?x=1", Path::new("")), "a b.txt");
        assert_eq!(suggested_name("https://example.com/", Path::new("report.pdf")), "report.pdf");
    }

    #[test]
    fn embedded_version_is_version_md() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../VERSION.md");
        let text = std::fs::read_to_string(path).expect("VERSION.md");
        let want = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with('#'))
            .expect("version line");
        assert_eq!(env!("MITHU_VERSION"), want);
    }

    #[test]
    fn ipc_status_field_unescapes() {
        let body = r#"{"kind":"status","status":"https://example.com/a"}"#;
        assert_eq!(json_field(body, "kind").as_deref(), Some("status"));
        assert_eq!(json_field(body, "status").as_deref(), Some("https://example.com/a"));
    }
}
