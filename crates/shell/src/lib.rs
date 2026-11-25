//! Servo page inside the shared Mithu frame.
//!
//! The page (DOM, style, layout, script, network) is Servo. The tab strip,
//! toolbar, bookmarks, and address bar are painted by `mithu-ui`. When the
//! window has a GL context, Servo paints into an offscreen framebuffer and
//! that frame is composited under the chrome on the GPU. Otherwise this crate
//! copies Servo's software framebuffer into the content rectangle.
//! No product logo is drawn.

mod present;

pub use present::Compositor;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Once};
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use mithu_text::{FontBook, Glyph, glyph_top};
use servo::{
    Code, DevicePoint, EventLoopWaker, ImeEvent, InputEvent, Key, KeyState, KeyboardEvent,
    LoadStatus, Location, Modifiers, MouseButton, MouseButtonAction, MouseButtonEvent,
    MouseMoveEvent, NamedKey, Opts, OutputOptions, PixelFormat, RenderingContext, Servo,
    ServoBuilder, SoftwareRenderingContext, Theme, WebView, WebViewBuilder, WebViewDelegate,
    WebViewPoint, WheelDelta, WheelEvent, WheelMode,
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

struct PendingPermission {
    origin: String,
    feature: String,
    request: servo::PermissionRequest,
}

#[derive(Clone)]
struct Delegate {
    frame: Rc<AtomicBool>,
    /// Stays set after the first paint of this navigation. `frame` is cleared
    /// on every spin, so a page that paints once and then goes quiet would
    /// never count as settled.
    painted: Rc<AtomicBool>,
    /// Webviews that produced a frame since the last spin. Only a frame from
    /// the focused tab dirties the window, as Chromium's compositor only
    /// draws visible surfaces.
    ready: Rc<RefCell<Vec<servo::WebViewId>>>,
    opened: Rc<RefCell<Vec<PendingTab>>>,
    shot: Rc<RefCell<Option<servo::RgbaImage>>>,
    saves: Rc<RefCell<Vec<String>>>,
    permissions: Rc<RefCell<Vec<PendingPermission>>>,
    /// Webviews whose script thread stopped answering. The tab shows
    /// "This page crashed" until a later load finishes with a title.
    crashed: Rc<RefCell<Vec<servo::WebViewId>>>,
    /// One rendering context shared by every tab, when the window has one.
    shared: Rc<RefCell<Option<Rc<dyn RenderingContext>>>>,
}

struct PendingTab {
    webview: WebView,
    context: Rc<dyn RenderingContext>,
}

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, webview: WebView) {
        // Do not paint here. `Browser::paint_due_frame` paints the focused
        // tab at most once per 60 Hz interval and never paints hidden tabs,
        // the way Chromium's cc scheduler only draws on a BeginFrame and
        // stops BeginFrames for occluded/hidden surfaces.
        self.ready.borrow_mut().push(webview.id());
        self.painted.store(true, Ordering::SeqCst);
    }

    fn request_create_new(&self, _parent: WebView, request: servo::CreateNewWebViewRequest) {
        let size = PhysicalSize::new(800, 600);
        let Some(context) = self.shared.borrow().clone().or_else(|| {
            SoftwareRenderingContext::new(size)
                .ok()
                .map(|context| Rc::new(context) as Rc<dyn RenderingContext>)
        }) else {
            return;
        };
        let dyn_ctx: Rc<dyn RenderingContext> = context.clone();
        let delegate: Rc<dyn WebViewDelegate> = Rc::new(self.clone());
        let webview = request.builder(dyn_ctx).delegate(delegate).build();
        webview.show();
        self.opened
            .borrow_mut()
            .push(PendingTab { webview, context });
    }

    fn request_navigation(&self, _webview: WebView, request: servo::NavigationRequest) {
        let url = request.url.to_string();
        if mithu_ui::pages::is_download_url(&url) {
            self.saves.borrow_mut().push(url);
            request.deny();
        }
    }

    fn request_permission(&self, webview: WebView, request: servo::PermissionRequest) {
        let origin = webview
            .url()
            .and_then(|url| url.host_str().map(|host| host.to_string()))
            .unwrap_or_default();
        let feature = match request.feature() {
            servo::PermissionFeature::Geolocation => "location",
            servo::PermissionFeature::Notifications => "notifications",
            servo::PermissionFeature::Camera => "camera",
            servo::PermissionFeature::Microphone => "microphone",
            servo::PermissionFeature::BackgroundSync => "background",
            servo::PermissionFeature::PersistentStorage => "storage",
            _ => {
                request.deny();
                return;
            }
        };
        self.permissions.borrow_mut().push(PendingPermission {
            origin,
            feature: feature.to_string(),
            request,
        });
        self.frame.store(true, Ordering::SeqCst);
    }

    fn notify_crashed(&self, webview: WebView, _reason: String, _backtrace: Option<String>) {
        let id = webview.id();
        let mut crashed = self.crashed.borrow_mut();
        if !crashed.contains(&id) {
            crashed.push(id);
        }
        drop(crashed);
        self.frame.store(true, Ordering::SeqCst);
    }
}

struct Tab {
    webview: WebView,
    context: Rc<dyn RenderingContext>,
    title: String,
    url: String,
    status: LoadStatus,
    zoom: f32,
    group_id: u8,
    /// `MITHU_BENCH` printed this tab's first complete load.
    bench_reported: bool,
    /// Servo has painted this tab's surface at least once, so reading it
    /// back gives page pixels rather than an uninitialised buffer.
    has_frame: bool,
    /// When this tab was last the focused tab (discard order).
    last_active: Instant,
    /// Last paint while hidden (rate limit).
    /// Set while the tab is discarded: the URL to load when it is selected.
    discarded: Option<Url>,
}

/// Wakes the engine thread when Servo has work, the same role as Chrome's
/// task-runner wake on the browser UI thread.
pub struct EngineWake {
    pending: Mutex<bool>,
    cv: Condvar,
    /// Wakes the winit loop. Set by the window thread; screenshot mode leaves it empty.
    notifier: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl EngineWake {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::new(false),
            cv: Condvar::new(),
            notifier: Mutex::new(None),
        })
    }

    pub fn set_notifier(&self, notifier: impl Fn() + Send + Sync + 'static) {
        *self.notifier.lock().expect("engine wake") = Some(Arc::new(notifier));
    }

    pub fn poke(&self) {
        let mut pending = self.pending.lock().expect("engine wake");
        let fresh = !*pending;
        *pending = true;
        self.cv.notify_one();
        drop(pending);
        if fresh {
            if let Some(notifier) = self.notifier.lock().expect("engine wake").clone() {
                notifier();
            }
        }
    }

    /// Wait until [`Self::poke`] or `limit` elapses.
    pub fn wait(&self, limit: Duration) {
        let pending = self.pending.lock().expect("engine wake");
        if *pending {
            return;
        }
        let _ = self.cv.wait_timeout(pending, limit);
    }

    pub fn take(&self) -> bool {
        let mut pending = self.pending.lock().expect("engine wake");
        let was = *pending;
        *pending = false;
        was
    }
}

struct ServoWake(Arc<EngineWake>);

impl EventLoopWaker for ServoWake {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(ServoWake(self.0.clone()))
    }

    fn wake(&self) {
        self.0.poke();
    }
}

thread_local! {
    static ENGINE_WAKE: RefCell<Option<Arc<EngineWake>>> = const { RefCell::new(None) };
}

/// The engine thread calls this before [`Browser::new`] so Servo can wake it.
pub fn bind_engine_wake(wake: Arc<EngineWake>) {
    ENGINE_WAKE.with(|slot| *slot.borrow_mut() = Some(wake));
}

/// The channel-disconnect test stays in-process. A content-process re-exec
/// needs a harness that understands `--content-process`, and Servo's drop
/// waits on that child.
static STAY_IN_PROCESS: AtomicBool = AtomicBool::new(false);

/// Keep every page on the in-process script thread. For the regression test.
pub fn stay_in_process() {
    STAY_IN_PROCESS.store(true, Ordering::Relaxed);
}

/// Child processes spawned for a site re-exec this binary. Servo passes
/// `--content-process` and a channel token, the same shape as a Chromium renderer.
pub fn run_content_process_if_requested() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("--content-process") {
        if let Some(token) = args.next() {
            servo::run_content_process(token);
            std::process::exit(0);
        }
    }
}

#[derive(Clone)]
struct TabGroup {
    id: u8,
    name: String,
    color: u8,
    collapsed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    None,
    WindowName,
    GroupName,
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
    Page,
    Tab,
    Omnibox,
    Link,
    Image,
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
    submenu: Option<mithu_ui::menus::MenuId>,
    quit: bool,
    fullscreen_toggle: bool,
    window_private: Option<bool>,
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
    scale: f32,
    prefs: mithu_ui::pages::Prefs,
    handled_internal: String,
    fonts: FontBook,
    revision: u64,
    frame: Rc<AtomicBool>,
    painted: Rc<AtomicBool>,
    status: String,
    groups: Vec<TabGroup>,
    group_editor: bool,
    window_name: String,
    field: Field,
    permission_prompt: String,
    visibility_stamp: Option<(usize, usize, bool)>,
    /// Last page pixels read from the focused tab's surface (0RGB rows).
    /// Chrome-only repaints (hover, typing in the omnibox) reuse it instead
    /// of reading the GL surface back again.
    page_cache: Vec<u32>,
    page_cache_dims: (u32, u32),
    page_cache_id: Option<servo::WebViewId>,
    page_dirty: bool,
    /// `--screenshot` waits for Servo's screenshot readiness (images, fonts,
    /// load event). The interactive window never does.
    exact_capture: bool,
    /// Set on a tab switch: the first window frame of the newly shown tab
    /// goes through the screenshot path, which waits for Servo to rebuild
    /// the shown pipeline's display list (paint holding: the old pixels stay
    /// until then) instead of reading back a surface that is still blank.
    switch_capture: bool,
    /// The focused tab has a new frame that has not been painted yet.
    paint_pending: bool,
    last_paint: Instant,
    /// After a tab switch, paint every interval until this time even without
    /// a new-frame notification: the first paint after `show()` can run
    /// before the compositor has the shown tab's display list.
    /// Animated wheel scrolling (Chromium's ease-in-out scroll curve).
    scroller: mithu_ui::scroll::ScrollAnimator,
    /// System memory pressure (Chromium's 5 s evaluator).
    memory: mithu_ui::memory::Monitor,
    /// Window GL compositor. Absent in screenshot mode and when no GL context
    /// could be created; those paths read the software framebuffer back.
    compositor: Option<Compositor>,
    /// One software context shared by every tab on the readback path.
    software: Option<Rc<SoftwareRenderingContext>>,
    /// Chrome pixels changed since the last GPU present.
    chrome_dirty: bool,
    /// Page frames since launch. Added into [`Self::content_revision`] so a
    /// new page frame still wakes the software window.
    frame_seq: u64,
    /// Backs off painting when sampled scanlines stop changing.
    stable_level: u32,
    last_hash: Option<u64>,
    interacted: Instant,
    chrome_pixels: Vec<u32>,
    chrome_rgba: Vec<u8>,
    chrome_upload: bool,
    composited_once: bool,
}

impl Browser {
    pub fn new() -> Self {
        match Self::try_new() {
            Ok(browser) => browser,
            Err(error) => panic!("{error}"),
        }
    }

    pub fn try_new() -> Result<Self, String> {
        Self::build(None)
    }

    /// Headed window. `compositor` paints Servo straight into the window GL
    /// surface. `None` keeps the software readback path (also used when the
    /// caller passes a compositor that should be ignored).
    pub fn try_new_composited(compositor: Option<Compositor>) -> Result<Self, String> {
        let size = compositor.as_ref().map(Compositor::size);
        let mut browser = Self::build(compositor)?;
        if let Some((width, height)) = size {
            browser.resize(width as f32, height as f32);
        }
        Ok(browser)
    }

    pub fn uses_compositor(&self) -> bool {
        self.compositor.is_some()
    }

    fn build(compositor: Option<Compositor>) -> Result<Self, String> {
        install_crypto();
        let frame = Rc::new(AtomicBool::new(false));
        let painted = Rc::new(AtomicBool::new(false));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let delegate = Delegate {
            frame: frame.clone(),
            painted: painted.clone(),
            opened,
            shot: Rc::new(RefCell::new(None)),
            ready: Rc::new(RefCell::new(Vec::new())),
            saves: Rc::new(RefCell::new(Vec::new())),
            permissions: Rc::new(RefCell::new(Vec::new())),
            crashed: Rc::new(RefCell::new(Vec::new())),
            shared: Rc::new(RefCell::new(None)),
        };
        let config = mithu_config_dir();
        let _ = std::fs::create_dir_all(&config);
        let cores = std::thread::available_parallelism()
            .map(|count| count.get() as u64)
            .unwrap_or(4)
            .clamp(2, 8);
        let mut preferences = servo::Preferences::default();
        preferences.network_http_proxy_uri = String::new();
        preferences.network_https_proxy_uri = String::new();
        preferences.user_agent = strip_servo_token(&preferences.user_agent);
        // Stylo and the shared pools follow the machine, the way Chrome sizes
        // its thread pool and renderer compositor threads.
        preferences.layout_threads = cores;
        preferences.layout_parallelism_job_count_minimum = 2;
        preferences.thread_pool_workers_max = cores;
        preferences.thread_pool_async_runtime_workers_max = cores;
        preferences.thread_pool_fallback_workers = cores.min(4);
        preferences.thread_pool_webrender_workers_max = cores.min(4);
        preferences.thread_pool_canvas_workers = cores.min(4);
        preferences.js_offthread_compilation_enabled = true;
        // SpiderMonkey's cap is in megabytes and must be below 256.
        preferences.js_mem_max = 192;
        preferences.network_http_cache_size = 1024;
        preferences.network_http_disk_cache = config.join("http-cache.sqlite").display().to_string();
        preferences.network_http_disk_cache_size = 32 * 1024 * 1024;
        preferences.session_history_max_length = 8;
        let mut opts = Opts::default();
        opts.multiprocess = !STAY_IN_PROCESS.load(Ordering::Relaxed);
        opts.sandbox = false;
        // Servo's default is the test-runner mode: a pipeline panic calls
        // process::exit. A crashed page has to stay inside its tab.
        opts.hard_fail = false;
        opts.config_dir = Some(config);
        if let Ok(path) = std::env::var("MITHU_TIME_PROFILE") {
            if path == "-" {
                opts.time_profiling = Some(OutputOptions::Stdout(2.0));
            } else if !path.is_empty() {
                opts.time_profiling = Some(OutputOptions::FileName(path));
            }
        }
        let mut builder = ServoBuilder::default().preferences(preferences).opts(opts);
        if let Some(wake) = ENGINE_WAKE.with(|slot| slot.borrow().clone()) {
            builder = builder.event_loop_waker(Box::new(ServoWake(wake)));
        }
        let servo = builder.build();
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
            submenu: None,
            quit: false,
            fullscreen_toggle: false,
            window_private: None,
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
            scale: 1.0,
            prefs: mithu_ui::pages::load(&mithu_ui::pages::path()),
            handled_internal: String::new(),
            fonts: FontBook::load(),
            revision: 1,
            frame,
            painted,
            status: String::new(),
            groups: Vec::new(),
            group_editor: false,
            window_name: String::new(),
            field: Field::None,
            permission_prompt: String::new(),
            visibility_stamp: None,
            page_cache: Vec::new(),
            page_cache_dims: (0, 0),
            page_cache_id: None,
            page_dirty: true,
            exact_capture: false,
            switch_capture: false,
            scroller: mithu_ui::scroll::ScrollAnimator::new(),
            paint_pending: false,
            last_paint: Instant::now(),
            memory: mithu_ui::memory::Monitor::new(),
            compositor,
            software: None,
            chrome_dirty: true,
            frame_seq: 0,
            stable_level: 0,
            last_hash: None,
            interacted: Instant::now(),
            chrome_pixels: Vec::new(),
            chrome_rgba: Vec::new(),
            chrome_upload: true,
            composited_once: false,
        };
        browser.open_tab(start_url())?;
        browser.apply_prefs();
        browser.open_startup();
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
        self.revision.wrapping_add(self.frame_seq)
    }

    pub fn status_line(&self) -> String {
        self.status.clone()
    }

    pub fn script_logs(&self) -> Vec<String> {
        Vec::new()
    }

    pub fn title(&self) -> String {
        let page = self
            .tabs
            .get(self.active)
            .map(|tab| tab.title.clone())
            .unwrap_or_default();
        if self.window_name.is_empty() {
            page
        } else if page.is_empty() {
            self.window_name.clone()
        } else {
            format!("{} — {page}", self.window_name)
        }
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
            return done && have.starts_with("http") && self.painted.load(Ordering::SeqCst);
        }
        done && same_url(have, want)
    }

    pub fn hold_status(&self) -> String {
        let Some(tab) = self.tabs.get(self.active) else {
            return "no tab".into();
        };
        let state = match tab.status {
            LoadStatus::Started => "started",
            LoadStatus::HeadParsed => "head",
            LoadStatus::Complete => "complete",
            LoadStatus::Stopped => "stopped",
        };
        format!("{state} {}", tab.url)
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.viewport_w = width.max(1.0);
        self.viewport_h = height.max(1.0);
        if let Some(compositor) = &self.compositor {
            let window = PhysicalSize::new(
                self.viewport_w.round() as u32,
                self.viewport_h.round() as u32,
            );
            compositor.resize(window, self.content_physical());
        }
        self.resize_active();
        self.page_dirty = true;
        self.bump();
    }

    pub fn set_scale(&mut self, scale: f32) {
        let scale = if scale.is_finite() && scale >= 1.0 { scale } else { 1.0 };
        if (self.scale - scale).abs() > 0.01 {
            self.scale = scale;
            self.resize_active();
        }
    }

    pub fn navigate(&mut self, input: &str) {
        let trimmed = input.trim();
        if trimmed.starts_with("mithu:") || (trimmed.starts_with("about:") && trimmed != "about:blank") {
            self.menu = None;
            self.omnibox_focused = false;
            self.open_shared_page(trimmed);
            return;
        }
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
        self.painted.store(false, Ordering::SeqCst);
        self.page_dirty = true;
        self.note_interaction();
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
        self.open_shared_page("mithu://newtab");
        self.omnibox_focused = false;
        self.tab_search = false;
        self.spin();
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
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
        self.field != Field::None
    }

    pub fn submit_field(&mut self) {
        self.field = Field::None;
        self.status = if self.window_name.is_empty() {
            "Window name cleared.".into()
        } else {
            format!("Window named {}.", self.window_name)
        };
        self.bump();
    }

    pub fn field_backspace(&mut self) {
        match self.field {
            Field::WindowName => {
                self.window_name.pop();
            }
            Field::GroupName => {
                if let Some(group) = self.active_group_mut() {
                    group.name.pop();
                }
            }
            Field::None => {}
        }
        self.bump();
    }

    pub fn field_insert(&mut self, ch: char) {
        if ch.is_control() {
            return;
        }
        match self.field {
            Field::WindowName => self.window_name.push(ch),
            Field::GroupName => {
                if let Some(group) = self.active_group_mut() {
                    if group.name == "Group" {
                        group.name.clear();
                    }
                    group.name.push(ch);
                }
            }
            Field::None => {}
        }
        self.bump();
    }

    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    pub fn close_menu(&mut self) {
        self.menu = None;
        self.submenu = None;
        self.bump();
    }

    pub fn consume_quit(&mut self) -> bool {
        let quit = self.quit;
        self.quit = false;
        quit
    }

    pub fn consume_fullscreen(&mut self) -> bool {
        let toggle = self.fullscreen_toggle;
        self.fullscreen_toggle = false;
        toggle
    }

    /// `Some(true)` asks the host for a private window. `Some(false)` asks for another normal window.
    pub fn take_window(&mut self) -> Option<bool> {
        self.window_private.take()
    }

    pub fn context_target(&mut self, kind: &str, x: f32, y: f32) {
        self.pointer = (x, y);
        self.menu = Some(match kind {
            "link" => MenuKind::Link,
            "image" => MenuKind::Image,
            "tab" => MenuKind::Tab,
            "omnibox" => MenuKind::Omnibox,
            _ => MenuKind::Page,
        });
        self.submenu = None;
        self.bump();
    }

    pub fn context_menu(&mut self, x: f32, y: f32) {
        self.pointer = (x, y);
        let scale = if self.scale.is_finite() && self.scale >= 1.0 { self.scale } else { 1.0 };
        let tab_h = (41.0 * scale).round();
        let tool_h = (48.0 * scale).round();
        self.menu = Some(if y < tab_h {
            MenuKind::Tab
        } else if y < tab_h + tool_h {
            MenuKind::Omnibox
        } else {
            MenuKind::Page
        });
        self.submenu = None;
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
            Click::GroupEditor => {
                self.group_editor = self.active_group_id() != 0;
                if self.group_editor {
                    self.field = Field::GroupName;
                    self.omnibox_focused = false;
                }
                self.bump();
            }
            Click::GroupCollapse => self.toggle_collapse(),
            Click::Ungroup => self.ungroup_active(),
            Click::CloseGroup => self.close_group(),
            Click::GroupColor(color) => self.set_group_color(color),
            Click::AllowPermission => self.answer_permission(true),
            Click::BlockPermission => self.answer_permission(false),
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
                self.submenu = None;
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
            Click::MenuItem(item) => {
                if mithu_ui::menus::children(item).is_empty() {
                    self.menu = None;
                    self.submenu = None;
                    self.run_item(item);
                } else {
                    self.submenu = Some(item);
                    self.bump();
                }
            }
            Click::SubmenuItem(index) => {
                let parent = self.submenu;
                self.menu = None;
                self.submenu = None;
                if let Some(parent) = parent {
                    if let Some(row) = mithu_ui::menus::children(parent).get(index) {
                        let id = row.id;
                        self.run_item(id);
                    }
                }
            }
            Click::Zoom(kind) => {
                self.menu = None;
                self.submenu = None;
                match kind {
                    -1 => self.zoom_out(),
                    1 => self.zoom_in(),
                    2 => {
                        self.fullscreen_toggle = true;
                        self.bump();
                    }
                    _ => self.zoom_reset(),
                }
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

    /// A mouse-wheel notch: animate `dy` px over a few frames instead of
    /// jumping (see [`mithu_ui::scroll`]). Touchpad pixel deltas use
    /// [`Self::scroll_by`].
    pub fn scroll_animated(&mut self, dy: f32) {
        if self.overlay.is_some() {
            return;
        }
        if !self.prefs.smooth_scroll {
            self.wheel(0.0, -f64::from(dy));
            return;
        }
        self.scroller.add(dy, Instant::now());
        self.tick_scroll();
    }

    /// Emit this frame's share of any running wheel animation.
    pub fn tick_scroll(&mut self) {
        let delta = self.scroller.tick(Instant::now());
        if delta != 0.0 {
            self.wheel(0.0, -f64::from(delta));
        }
    }

    /// Time until the next scroll animation frame, if one is running. The
    /// engine thread uses it as its begin-frame deadline (60 Hz), like
    /// Chromium's timer-based `DelayBasedBeginFrameSource` when there is no
    /// vsync signal.
    pub fn scroll_frame_due(&self) -> Option<Duration> {
        self.scroller
            .next_frame(Instant::now(), Duration::from_micros(16_667))
    }

    pub fn scroll_page(&mut self, pages: f32) {
        let height = (self.viewport_h - self.content_top()).max(1.0);
        self.wheel(0.0, -f64::from(pages * height));
    }

    pub fn send_key(&mut self, kind: KeyKind, pressed: bool, ctrl: bool, alt: bool, shift: bool) {
        if self.omnibox_focused || self.find_focused {
            return;
        }
        self.note_interaction();
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

    /// Present the page framebuffer and the chrome texture. Returns false when
    /// there is no GL compositor or nothing has changed.
    pub fn composite(&mut self) -> bool {
        if self.compositor.is_none() {
            return false;
        }
        if self.composited_once && !self.page_dirty && !self.chrome_dirty {
            return false;
        }
        let width = self.viewport_w.round().max(1.0) as u32;
        let height = self.viewport_h.round().max(1.0) as u32;
        let count = (width as usize) * (height as usize);
        if self.chrome_dirty || self.chrome_pixels.len() != count {
            let mut pixels = vec![0u32; count];
            if self.overlay.is_some() {
                self.draw_overlay(&mut pixels, width, height);
            }
            self.draw_chrome(&mut pixels, width, height);
            self.chrome_pixels = pixels;
            self.chrome_rgba = mithu_composite::chrome_rgba(&self.chrome_pixels);
            self.chrome_upload = true;
        }
        let top = self.content_top().round().max(0.0) as i32;
        let blit_page = self.overlay.is_none()
            && self
                .tabs
                .get(self.active)
                .is_some_and(|tab| tab.has_frame && tab.discarded.is_none());
        let upload = self.chrome_upload;
        let mut compositor = self.compositor.take().expect("compositor");
        compositor.present(&self.chrome_rgba, width, height, top, blit_page, upload);
        self.compositor = Some(compositor);
        self.page_dirty = false;
        self.chrome_dirty = false;
        self.chrome_upload = false;
        self.composited_once = true;
        true
    }

    fn page_context(&mut self) -> Result<Rc<dyn RenderingContext>, String> {
        if let Some(compositor) = &self.compositor {
            return Ok(compositor.page_context());
        }
        if self.software.is_none() {
            self.software = Some(Rc::new(software_context(self.content_physical())?));
        }
        let context = self.software.clone().expect("software context");
        Ok(context)
    }

    fn open_tab(&mut self, url: Url) -> Result<(), String> {
        let context = self.page_context()?;
        *self.delegate.shared.borrow_mut() = Some(context.clone());
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
            group_id: 0,
            bench_reported: false,
            has_frame: false,
            last_active: Instant::now(),
            discarded: None,
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
                group_id: 0,
                bench_reported: false,
                has_frame: false,
                last_active: Instant::now(),
                discarded: None,
            });
            self.active = self.tabs.len() - 1;
            self.bump();
        }
    }

    pub fn spin_once(&mut self) {
        self.spin();
    }

    fn spin(&mut self) {
        if let Some(tab) = self.tabs.get(self.active) {
            let _ = tab.context.make_current();
        }
        self.servo.spin_event_loop();
        self.adopt_pending();
        self.drain_downloads();
        self.drain_permissions();
        self.sync_active();
        self.sync_visibility();
        self.drain_ready_frames();
        self.paint_due_frame();
        self.relieve_memory_pressure();
        if self.frame.swap(false, Ordering::SeqCst) {
            self.bump();
        }
        if mithu_host::bench_enabled() {
            self.bench_report();
        }
    }

    /// Discard one background tab when the system is short of memory
    /// (see [`mithu_ui::memory`]).
    fn relieve_memory_pressure(&mut self) {
        if !self.prefs.memory_saver {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.last_active = Instant::now();
        }
        if self.tabs.len() < 2 {
            return;
        }
        let now = Instant::now();
        let Some(level) = self.memory.poll(now) else {
            return;
        };
        let candidates: Vec<mithu_ui::memory::Candidate> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| mithu_ui::memory::Candidate {
                active: index == self.active,
                discarded: tab.discarded.is_some(),
                loaded: tab.has_frame || tab.webview.load_status() == LoadStatus::Complete,
                last_active: tab.last_active,
            })
            .collect();
        if let Some(index) = mithu_ui::memory::pick_discard(&candidates, level, now) {
            self.discard_tab(index, level);
        }
    }

    /// Chromium's tab discard: drop the page (and its content process once no
    /// other tab of that site needs it) but keep the tab, its title, and its
    /// URL. Selecting the tab loads the URL again.
    fn discard_tab(&mut self, index: usize, level: mithu_ui::memory::Pressure) {
        if index == self.active || index >= self.tabs.len() {
            return;
        }
        let current = self.tabs[index].webview.url();
        let Some(url) = current.or_else(|| Url::parse(&self.tabs[index].url).ok()) else {
            return;
        };
        if !url.scheme().starts_with("http") {
            return;
        }
        let Ok(context) = self.page_context() else {
            return;
        };
        let dyn_ctx: Rc<dyn RenderingContext> = context.clone();
        let delegate: Rc<dyn WebViewDelegate> = Rc::new(self.delegate.clone());
        // An about:blank webview stays on the in-process event loop, so the
        // placeholder does not start a content process.
        let placeholder = WebViewBuilder::new(&self.servo, dyn_ctx)
            .delegate(delegate)
            .url(start_url())
            .hidpi_scale_factor(euclid_scale())
            .build();
        placeholder.hide();
        let tab = &mut self.tabs[index];
        let old = std::mem::replace(&mut tab.webview, placeholder);
        tab.context = context;
        tab.has_frame = false;
        tab.discarded = Some(url.clone());
        drop(old);
        if mithu_host::bench_enabled() {
            eprintln!("mithu: discarded tab {index} ({url}) under {level:?} memory pressure");
        }
    }

    /// Reload a discarded tab when it becomes the focused tab.
    fn restore_discarded_active(&mut self) {
        let size = self.content_physical();
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let Some(url) = tab.discarded.take() else {
            return;
        };
        tab.webview.resize(size);
        tab.webview.load(url);
    }

    fn drain_ready_frames(&mut self) {
        let ready = std::mem::take(&mut *self.delegate.ready.borrow_mut());
        if ready.is_empty() {
            return;
        }
        let active = self.tabs.get(self.active).map(|tab| tab.webview.id());
        for id in &ready {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.webview.id() == *id) {
                tab.has_frame = true;
            }
        }
        if active.is_some_and(|active| ready.contains(&active)) {
            self.paint_pending = true;
        }
    }

    /// One frame interval at 60 Hz. Mithu has no vsync signal (it presents
    /// through softbuffer), so this is Chromium's
    /// `viz::DelayBasedBeginFrameSource` fallback: a timer at the display
    /// rate.
    const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

    /// Paint the focused tab if it has a new frame and a frame interval has
    /// passed since the last paint. Servo was producing ~120 frames a second
    /// on an idle Wikipedia page and every one was rasterised; now at most 60
    /// are, and fewer when the sampled scanlines stop changing or a load is
    /// still parsing.
    fn paint_due_frame(&mut self) {
        if !self.paint_pending || self.last_paint.elapsed() < self.current_paint_interval() {
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let _ = tab.context.make_current();
        tab.webview.paint();
        let loaded = tab.webview.load_status() == LoadStatus::Complete;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.has_frame |= loaded;
        }
        if mithu_host::bench_enabled() {
            mithu_host::bench_count_frames(1);
        }
        self.last_paint = Instant::now();
        self.paint_pending = false;
        self.page_dirty = true;
        self.frame_seq = self.frame_seq.wrapping_add(1);
        let interacting = self.interacted.elapsed() < Duration::from_millis(400);
        if interacting {
            self.stable_level = 0;
        } else if let Some(hash) = self.sample_hash() {
            if self.last_hash == Some(hash) {
                self.stable_level = self.stable_level.saturating_add(1).min(8);
            } else {
                self.stable_level = 0;
            }
            self.last_hash = Some(hash);
        }
    }

    /// Time until a pending frame may be painted.
    pub fn paint_due(&self) -> Option<Duration> {
        self.paint_pending
            .then(|| self.current_paint_interval().saturating_sub(self.last_paint.elapsed()))
    }

    /// Deadline for the window thread's next wake.
    pub fn next_wake(&self) -> Duration {
        let busy = !self.load_settled()
            || self.animation_frames_pending()
            || self.paint_pending
            || self.stable_level > 0;
        let mut limit = if busy {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(1000)
        };
        if let Some(due) = self.scroll_frame_due() {
            limit = limit.min(due);
        }
        if let Some(due) = self.paint_due() {
            limit = limit.min(due);
        }
        limit.max(Duration::from_millis(1))
    }

    fn current_paint_interval(&self) -> Duration {
        let loading = self.tabs.get(self.active).is_some_and(|tab| {
            matches!(tab.status, LoadStatus::Started | LoadStatus::HeadParsed)
        });
        let interacting = self.interacted.elapsed() < Duration::from_millis(400);
        mithu_composite::paint_interval(loading, self.stable_level, interacting)
    }

    fn note_interaction(&mut self) {
        self.interacted = Instant::now();
        self.stable_level = 0;
    }

    /// Hash a few full-width scanlines. A page that keeps asking for frames
    /// but is not actually changing pixels backs off instead of rasterising
    /// at 60 Hz.
    fn sample_hash(&self) -> Option<u64> {
        let tab = self.tabs.get(self.active)?;
        let size = tab.context.size();
        let width = size.width as i32;
        let height = size.height as i32;
        if width < 2 || height < 8 {
            return None;
        }
        let _ = tab.context.make_current();
        let mut hash = 0xcbf29ce484222325u64;
        for frac in [1, 3, 5, 7] {
            let y = (height * frac / 8).clamp(0, height - 1);
            let rect = servo::DeviceIntRect::from_origin_and_size(
                servo::DeviceIntPoint::new(0, y),
                servo::DeviceIntSize::new(width, 1),
            );
            let image = tab.context.read_to_image(rect)?;
            for byte in image.as_raw().iter().step_by(16) {
                hash = hash.wrapping_mul(0x100000001b3).wrapping_add(*byte as u64);
            }
        }
        Some(hash)
    }

    /// Use Servo's screenshot path (waits for images, fonts, and the load
    /// event) instead of reading the last painted frame.
    pub fn set_exact_capture(&mut self, on: bool) {
        self.exact_capture = on;
    }

    fn bench_report(&mut self) {
        for tab in &mut self.tabs {
            if tab.bench_reported || tab.webview.load_status() != LoadStatus::Complete {
                continue;
            }
            let Some(url) = tab.webview.url() else {
                continue;
            };
            if url.scheme().starts_with("http") {
                tab.bench_reported = true;
                mithu_host::bench_report_loaded(url.as_str());
            }
        }
    }

    fn sync_active(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if tab.discarded.is_some() {
            // Still the about:blank placeholder; the reload starts in
            // `sync_visibility`, keep the remembered title and URL until then.
            return;
        }
        let status = tab.webview.load_status();
        let url = tab
            .webview
            .url()
            .map(|url| url.to_string())
            .unwrap_or_default();
        let id = tab.webview.id();
        let page_title = tab.webview.page_title().filter(|title| !title.is_empty());
        let mut crashed_ids = self.delegate.crashed.borrow_mut();
        if crashed_ids.contains(&id) && status == LoadStatus::Complete && page_title.is_some() {
            crashed_ids.retain(|item| *item != id);
        }
        let crashed = crashed_ids.contains(&id);
        drop(crashed_ids);
        let title = if crashed {
            "This page crashed".to_string()
        } else {
            page_title.unwrap_or_else(|| display_title(&url))
        };
        let changed = tab.status != status || tab.url != url || tab.title != title;
        let link = tab.webview.status_text().unwrap_or_default();
        tab.status = status;
        if tab.url != url && url.starts_with("http") && !self.visits.contains(&url) {
            self.visits.push(url.clone());
            self.prefs.remember(&url, &title);
            mithu_ui::pages::save(&mithu_ui::pages::path(), &self.prefs);
        }
        let follow = if url.starts_with("mithu://") && url != self.handled_internal {
            Some(url.clone())
        } else {
            None
        };
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
        if let Some(spec) = follow {
            self.handled_internal = spec.clone();
            self.open_shared_page(&spec);
        }
    }

    fn open_shared_page(&mut self, spec: &str) {
        if let Some(command) = mithu_ui::pages::command(spec, &mut self.prefs) {
            mithu_ui::pages::save(&mithu_ui::pages::path(), &self.prefs);
            self.apply_prefs();
            if let Some(path) = &command.open_path {
                reveal_path(path, false);
            }
            if let Some(path) = &command.reveal_path {
                reveal_path(path, true);
            }
            self.present_document(&command.redirect);
            return;
        }
        if mithu_ui::pages::is_internal(spec) {
            self.present_document(spec);
        }
    }

    fn present_document(&mut self, spec: &str) {
        self.publish_tasks();
        let Some(document) = mithu_ui::pages::open(spec, &self.prefs, env!("MITHU_VERSION")) else {
            return;
        };
        self.overlay = None;
        self.omnibox = document.url.clone();
        self.omnibox_focused = false;
        let url = write_page(&document);
        self.pending_url = Some(url.to_string());
        self.page_dirty = true;
        self.note_interaction();
        if let Some(tab) = self.tabs.get(self.active) {
            tab.webview.load(url);
        }
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.url = document.url;
            tab.title = document.title;
        }
        self.bump();
    }

    fn open_startup(&mut self) {
        match self.prefs.startup.as_str() {
            "continue" => {
                if let Some(url) = self.prefs.history.last().map(|visit| visit.url.clone()) {
                    self.navigate(&url);
                    return;
                }
            }
            "pages" => {
                if let Some(url) = self.prefs.startup_pages.first().cloned() {
                    self.navigate(&url);
                    return;
                }
            }
            _ => {}
        }
        self.open_shared_page("mithu://newtab");
    }

    fn apply_prefs(&mut self) {
        self.dark = self.prefs.dark();
        self.bookmarks_open = self.prefs.show_bookmarks;
        mithu_ui::pages::set_edition("Mithu");
        mithu_ui::pages::sync_runtime(&self.prefs);
        self.bookmarks = self
            .prefs
            .bookmarks
            .iter()
            .map(|mark| Bookmark { title: mark.title.clone(), url: mark.url.clone() })
            .collect();
        self.apply_theme();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.zoom = self.prefs.zoom;
            tab.webview.set_page_zoom(self.prefs.zoom);
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
        chrome::chrome_height(self.bookmarks_open, self.scale)
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
        self.note_interaction();
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

    fn run_item(&mut self, item: mithu_ui::menus::MenuId) {
        use mithu_ui::menus::MenuId;
        match item {
            MenuId::Back => self.back(),
            MenuId::Forward => self.forward(),
            MenuId::Reload => self.reload(),
            MenuId::NewTab | MenuId::OpenLinkTab | MenuId::OpenImageTab => self.new_tab(),
            MenuId::NewWindow | MenuId::OpenLinkWindow => {
                self.window_private = Some(false);
                self.bump();
            }
            MenuId::PrivateWindow | MenuId::OpenLinkPrivate => {
                self.window_private = Some(true);
                self.bump();
            }
            MenuId::Print => self.print(),
            MenuId::Translate => self.navigate("mithu://translate"),
            MenuId::DevTools => self.navigate("mithu://inspect"),
            MenuId::Bookmark | MenuId::BookmarkManager => {
                if item == MenuId::BookmarkManager {
                    self.navigate("mithu://bookmarks");
                } else {
                    self.toggle_bookmark();
                }
            }
            MenuId::ShowBookmarksBar => self.toggle_bookmark_list(),
            MenuId::History => self.navigate("mithu://history"),
            MenuId::Bookmarks => self.navigate("mithu://bookmarks"),
            MenuId::Downloads => self.navigate("mithu://downloads"),
            MenuId::Settings | MenuId::Help => self.navigate("mithu://settings"),
            MenuId::About => self.navigate("mithu://version"),
            MenuId::Passwords | MenuId::PasswordManager | MenuId::Payments | MenuId::Addresses => {
                self.navigate("mithu://settings/autofill");
            }
            MenuId::Extensions | MenuId::ManageExtensions => self.navigate("mithu://extensions"),
            MenuId::DeleteData => self.navigate("mithu://settings/privacy"),
            MenuId::Performance => self.navigate("mithu://settings/performance"),
            MenuId::ZoomIn => self.zoom_in(),
            MenuId::ZoomOut => self.zoom_out(),
            MenuId::ZoomReset => self.zoom_reset(),
            MenuId::Find | MenuId::FindNext => self.open_find(),
            MenuId::CloseTab => self.close_tab(),
            MenuId::ReopenClosed => self.reopen_closed_tab(),
            MenuId::DuplicateTab => {
                let url = self.location();
                self.new_tab();
                if url.starts_with("http") {
                    self.navigate(&url);
                }
            }
            MenuId::CloseOther => {
                let keep = self.location();
                if self.tabs.iter().any(|tab| tab.url == keep) {
                    self.tabs.retain(|tab| tab.url == keep);
                    self.active = 0;
                    self.resize_active();
                    self.sync_active();
                    self.bump();
                }
            }
            MenuId::Exit => self.quit = true,
            MenuId::Fullscreen => self.fullscreen_toggle = true,
            MenuId::CopyLink | MenuId::CopyLinkAddress | MenuId::CopyImage => self.copy_location(item),
            MenuId::Copy => self.edit_command("copy"),
            MenuId::Cut => self.edit_command("cut"),
            MenuId::Paste => self.edit_command("paste"),
            MenuId::SelectAll => self.edit_command("selectAll"),
            MenuId::PasteAndGo => self.paste_and_go(),
            MenuId::SavePage | MenuId::SaveLink | MenuId::SaveImage => self.save_active_page(),
            MenuId::CreateShortcut => self.create_shortcut(),
            MenuId::NameWindow => self.begin_window_name(),
            MenuId::SendDevice => {
                mithu_ui::clipboard::set_text(&self.location());
                self.status = "Link copied. No other Mithu devices are paired.".into();
                self.bump();
            }
            MenuId::TaskManager => self.navigate("mithu://tasks"),
            MenuId::NewTabGroup => self.new_group(),
            MenuId::CollapseGroup => self.toggle_collapse(),
            MenuId::CloseGroup => self.close_group(),
            MenuId::Zoom | MenuId::TabGroups | MenuId::SaveShare | MenuId::MoreTools | MenuId::Separator => {}
        }
    }

    fn publish_tasks(&self) {
        let rows = self
            .tabs
            .iter()
            .map(|tab| (tab.title.clone(), tab.url.clone(), tab.group_id))
            .collect();
        mithu_ui::pages::set_tasks(rows);
    }

    fn save_active_page(&mut self) {
        let url = self.location();
        let dir = if self.prefs.download_dir.is_empty() {
            mithu_ui::pages::downloads_folder()
        } else {
            std::path::PathBuf::from(&self.prefs.download_dir)
        };
        match mithu_ui::pages::save_remote(&url, &dir) {
            Ok(path) => {
                let shown = path.display().to_string();
                self.prefs.note_download(&url, &shown, "done");
                mithu_ui::pages::save(&mithu_ui::pages::path(), &self.prefs);
                self.downloads.push(shown.clone());
                self.status = format!("Saved {shown}");
            }
            Err(error) => self.status = error,
        }
        self.bump();
    }

    fn new_group(&mut self) {
        let id = self.groups.iter().map(|group| group.id).max().unwrap_or(0).saturating_add(1);
        let color = ((id - 1) % 8) + 1;
        self.groups.push(TabGroup {
            id,
            name: "Group".into(),
            color,
            collapsed: false,
        });
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.group_id = id;
        }
        self.group_editor = true;
        self.field = Field::GroupName;
        self.omnibox_focused = false;
        self.bump();
    }

    fn toggle_collapse(&mut self) {
        if let Some(group) = self.active_group_mut() {
            group.collapsed = !group.collapsed;
        }
        self.bump();
    }

    fn close_group(&mut self) {
        let id = self.active_group_id();
        if id == 0 {
            return;
        }
        self.tabs.retain(|tab| tab.group_id != id);
        self.groups.retain(|group| group.id != id);
        if self.tabs.is_empty() {
            let _ = self.open_tab(start_url());
        }
        self.active = self.active.min(self.tabs.len() - 1);
        self.group_editor = false;
        self.field = Field::None;
        self.resize_active();
        self.sync_active();
        self.bump();
    }

    fn ungroup_active(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.group_id = 0;
        }
        self.groups.retain(|group| self.tabs.iter().any(|tab| tab.group_id == group.id));
        self.group_editor = false;
        self.field = Field::None;
        self.bump();
    }

    fn active_group_id(&self) -> u8 {
        self.tabs.get(self.active).map(|tab| tab.group_id).unwrap_or(0)
    }

    fn active_group_mut(&mut self) -> Option<&mut TabGroup> {
        let id = self.active_group_id();
        if id == 0 {
            return None;
        }
        self.groups.iter_mut().find(|group| group.id == id)
    }

    fn set_group_color(&mut self, color: u8) {
        if let Some(group) = self.active_group_mut() {
            group.color = color.clamp(1, 8);
        }
        self.bump();
    }

    fn answer_permission(&mut self, allow: bool) {
        let pending = self.delegate.permissions.borrow_mut().pop();
        if let Some(pending) = pending {
            let value = format!(
                "{}:{}:{}",
                pending.origin,
                pending.feature,
                if allow { "allow" } else { "block" }
            );
            // Reuse the prefs setter through a command so the file format stays one path.
            let _ = mithu_ui::pages::command(
                &format!(
                    "mithu://do?op=set&key=exception&value={value}&back=mithu://settings/site/{}",
                    pending.feature
                ),
                &mut self.prefs,
            );
            mithu_ui::pages::save(&mithu_ui::pages::path(), &self.prefs);
            if allow {
                pending.request.allow();
            } else {
                pending.request.deny();
            }
        }
        self.permission_prompt.clear();
        self.bump();
    }

    fn drain_downloads(&mut self) {
        let urls: Vec<String> = self.delegate.saves.borrow_mut().drain(..).collect();
        for url in urls {
            self.save_url(&url);
        }
    }

    fn drain_permissions(&mut self) {
        if !self.permission_prompt.is_empty() {
            return;
        }
        let pending = self.delegate.permissions.borrow_mut().pop();
        let Some(pending) = pending else {
            return;
        };
        let choice = mithu_ui::pages::permission_for(&self.prefs, &pending.origin, &pending.feature);
        match choice.as_str() {
            "allow" => pending.request.allow(),
            "block" => pending.request.deny(),
            _ => {
                self.permission_prompt = format!(
                    "{} wants {}",
                    if pending.origin.is_empty() { "This page" } else { pending.origin.as_str() },
                    pending.feature
                );
                self.delegate.permissions.borrow_mut().push(pending);
            }
        }
        self.bump();
    }

    fn save_url(&mut self, url: &str) {
        let dir = if self.prefs.download_dir.is_empty() {
            mithu_ui::pages::downloads_folder()
        } else {
            std::path::PathBuf::from(&self.prefs.download_dir)
        };
        match mithu_ui::pages::save_remote(url, &dir) {
            Ok(path) => {
                let shown = path.display().to_string();
                self.prefs.note_download(url, &shown, "done");
                mithu_ui::pages::save(&mithu_ui::pages::path(), &self.prefs);
                self.downloads.push(shown.clone());
                self.status = format!("Saved {shown}");
            }
            Err(error) => self.status = error,
        }
        self.bump();
    }

    fn edit_command(&mut self, command: &str) {
        if self.omnibox_focused {
            match command {
                "copy" => mithu_ui::clipboard::set_text(if self.select_all { &self.omnibox } else { "" }),
                "cut" => {
                    mithu_ui::clipboard::set_text(if self.select_all { &self.omnibox } else { "" });
                    if self.select_all {
                        self.omnibox.clear();
                        self.cursor = 0;
                        self.select_all = false;
                    }
                }
                "paste" => {
                    let text = mithu_ui::clipboard::get_text();
                    if self.select_all {
                        self.omnibox.clear();
                        self.cursor = 0;
                        self.select_all = false;
                    }
                    for ch in text.chars().take(2000) {
                        self.insert_char(ch);
                    }
                }
                "selectAll" => self.select_all = true,
                _ => {}
            }
            self.bump();
            return;
        }
        if let Some(tab) = self.tabs.get(self.active) {
            let script = format!("document.execCommand('{command}')");
            tab.webview.evaluate_javascript(script, |_| {});
        }
        self.status = format!("{command} sent to the page.");
        self.bump();
    }

    fn paste_and_go(&mut self) {
        let text = mithu_ui::clipboard::get_text();
        if !text.trim().is_empty() {
            self.navigate(text.trim());
        }
    }

    fn copy_location(&mut self, item: mithu_ui::menus::MenuId) {
        let text = if item == mithu_ui::menus::MenuId::CopyLinkAddress && self.link_status.starts_with("http") {
            self.link_status.clone()
        } else {
            self.location()
        };
        mithu_ui::clipboard::set_text(&text);
        self.status = "Copied.".into();
        self.bump();
    }

    fn create_shortcut(&mut self) {
        let url = self.location();
        let title = self.tabs.get(self.active).map(|tab| tab.title.clone()).unwrap_or_else(|| "Mithu".into());
        let dir = mithu_ui::pages::downloads_folder();
        let name = title.chars().map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' { ch } else { '_' }).take(40).collect::<String>();
        let name = if name.is_empty() { "mithu".into() } else { name };
        let path = dir.join(format!("{name}.url"));
        let body = format!("[InternetShortcut]\r\nURL={url}\r\n");
        match std::fs::write(&path, body) {
            Ok(()) => {
                self.prefs.note_download(&url, &path.display().to_string(), "done");
                mithu_ui::pages::save(&mithu_ui::pages::path(), &self.prefs);
                self.status = format!("Shortcut saved to {}", path.display());
            }
            Err(error) => self.status = error.to_string(),
        }
        self.bump();
    }

    fn begin_window_name(&mut self) {
        if self.window_name.is_empty() {
            self.window_name = self.tabs.get(self.active).map(|tab| tab.title.clone()).unwrap_or_default();
        }
        self.field = Field::WindowName;
        self.omnibox_focused = false;
        self.group_editor = false;
        self.status = "Type a window name, then press Enter.".into();
        self.bump();
    }

    pub fn print(&mut self) {
        if let Some(tab) = self.tabs.get(self.active) {
            tab.webview.evaluate_javascript("window.print()", |_| {});
        }
        self.status = "Print requested.".into();
        self.bump();
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
        self.chrome_dirty = true;
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
        while !done.get() && started.elapsed() < Duration::from_secs(3) {
            self.spin();
            std::thread::sleep(Duration::from_millis(1));
        }
        slot.borrow_mut().take()
    }

    fn blit_page(&mut self, pixels: &mut [u32], width: u32) {
        let internal = self.tabs.get(self.active).is_some_and(|tab| {
            let url = tab.url.as_str();
            url.starts_with("mithu:") || url.starts_with("about:") || url.starts_with("file:")
        });
        let switched = std::mem::take(&mut self.switch_capture)
            && self.tabs.get(self.active).is_some_and(|tab| {
                tab.discarded.is_none()
                    && tab.webview.load_status() == LoadStatus::Complete
            });
        // `take_screenshot` waits until every web font has loaded. Internal
        // pages embed those fonts, and the wait can expire with no image,
        // which used to leave the content rectangle blank.
        if (self.exact_capture || switched) && !internal {
            if let Some(image) = self.capture_page() {
                if !mithu_composite::rgba_is_blank(image.as_raw()) {
                    self.store_page(&image);
                }
            }
        }
        let cached = self.tabs.get(self.active).is_some_and(|tab| {
            self.page_cache_id == Some(tab.webview.id()) && !self.page_dirty && !self.page_cache.is_empty()
        });
        if !cached {
            self.refresh_page_cache();
        }
        let still_missing = self.tabs.get(self.active).is_some_and(|tab| {
            self.page_cache_id != Some(tab.webview.id()) || self.page_cache.is_empty()
        });
        if still_missing {
            if let Some(tab) = self.tabs.get(self.active) {
                let _ = tab.context.make_current();
                tab.webview.paint();
            }
            if let Some(tab) = self.tabs.get_mut(self.active) {
                tab.has_frame = true;
            }
            self.page_dirty = true;
            self.refresh_page_cache();
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if self.page_cache_id != Some(tab.webview.id()) {
            return;
        }
        let (iw, ih) = self.page_cache_dims;
        let top = self.content_top().round().max(0.0) as usize;
        let width = width as usize;
        let copy_w = (iw as usize).min(width);
        let rows = (ih as usize).min((self.viewport_h as usize).saturating_sub(top));
        for y in 0..rows {
            let dest = (top + y) * width;
            let src = y * iw as usize;
            if dest + copy_w > pixels.len() {
                break;
            }
            pixels[dest..dest + copy_w].copy_from_slice(&self.page_cache[src..src + copy_w]);
        }
    }

    /// Read the focused tab's surface straight after Servo painted it, the
    /// way a compositor presents its latest frame. Before, every window
    /// frame went through `take_screenshot`, which waits until every image,
    /// web font, and load event has finished, and spun the engine thread in
    /// 1 ms sleeps until then.
    fn refresh_page_cache(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let id = tab.webview.id();
        let size = tab.context.size();
        let fresh = !self.page_dirty
            && self.page_cache_id == Some(id)
            && self.page_cache_dims == (size.width, size.height);
        if fresh || !tab.has_frame {
            return;
        }
        let _ = tab.context.make_current();
        let rect = servo::DeviceIntRect::from_origin_and_size(
            servo::DeviceIntPoint::new(0, 0),
            servo::DeviceIntSize::new(size.width as i32, size.height as i32),
        );
        let Some(image) = tab.context.read_to_image(rect) else {
            return;
        };
        self.store_page(&image);
    }

    fn store_page(&mut self, image: &servo::RgbaImage) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let (iw, ih) = (image.width(), image.height());
        let raw = image.as_raw();
        self.page_cache.clear();
        self.page_cache.reserve((iw as usize) * (ih as usize));
        self.page_cache.extend(
            raw.chunks_exact(4)
                .map(|px| rgb(px[0] as u32, px[1] as u32, px[2] as u32)),
        );
        self.page_cache_dims = (iw, ih);
        self.page_cache_id = Some(tab.webview.id());
        self.page_dirty = false;
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
                    "Saved pages are listed below. A site download still needs the page's own save.",
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
        let ids: Vec<u8> = self.tabs.iter().map(|tab| tab.group_id).collect();
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let group = self.groups.iter().find(|group| group.id == tab.group_id);
                let head = tab.group_id != 0 && ids.iter().position(|id| *id == tab.group_id) == Some(index);
                chrome::TabPaint {
                    title: if tab.title.is_empty() { "New Tab".into() } else { tab.title.clone() },
                    favicon: tab.webview.favicon().map(|image| favicon_rgba(&image)),
                    group: group.map(|group| group.color).unwrap_or(0),
                    group_name: group.map(|group| group.name.clone()).unwrap_or_default(),
                    collapsed: group.is_some_and(|group| group.collapsed) && !head,
                }
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
            Some(MenuKind::Page | MenuKind::Tab | MenuKind::Omnibox | MenuKind::Link | MenuKind::Image) => Some(chrome::Popup::Context),
            None => None,
        };
        let rows = menu_rows(self.menu);
        let zoom = self.tabs.get(self.active).map(|tab| (tab.zoom * 100.0).round() as u32).unwrap_or(100);
        let (submenu_at, submenu_labels) = match self.submenu {
            Some(id) => (rows.iter().position(|row| row.id == id), mithu_ui::menus::labels(&mithu_ui::menus::children(id), zoom)),
            None => (None, Vec::new()),
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
            menu_labels: mithu_ui::menus::labels(&rows, zoom),
            submenu_at,
            submenu_labels,
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
            scale: self.scale,
            dark: self.prefs.dark(),
            show_home: self.prefs.show_home,
            group_editor: self.group_editor,
            permission_prompt: &self.permission_prompt,
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
            chrome::Hit::GroupBar(_) => Click::GroupEditor,
            chrome::Hit::GroupCollapse => Click::GroupCollapse,
            chrome::Hit::Ungroup => Click::Ungroup,
            chrome::Hit::CloseGroup => Click::CloseGroup,
            chrome::Hit::GroupColor(color) => Click::GroupColor(color),
            chrome::Hit::AllowPermission => Click::AllowPermission,
            chrome::Hit::BlockPermission => Click::BlockPermission,
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
            chrome::Hit::MenuItem(index) => menu_rows(self.menu)
                .get(index)
                .map(|row| Click::MenuItem(row.id))
                .unwrap_or(Click::None),
            chrome::Hit::SubmenuItem(index) => Click::SubmenuItem(index),
            chrome::Hit::Zoom(kind) => Click::Zoom(kind),
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
    GroupEditor,
    GroupCollapse,
    Ungroup,
    CloseGroup,
    GroupColor(u8),
    AllowPermission,
    BlockPermission,
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
    MenuItem(mithu_ui::menus::MenuId),
    SubmenuItem(usize),
    Zoom(i8),
    Theme,
    Content,
    None,
}

fn menu_rows(kind: Option<MenuKind>) -> Vec<mithu_ui::menus::Row> {
    match kind {
        Some(MenuKind::App) => mithu_ui::menus::app_menu(),
        Some(MenuKind::Page) => mithu_ui::menus::page_menu(),
        Some(MenuKind::Tab) => mithu_ui::menus::tab_menu(),
        Some(MenuKind::Omnibox) => mithu_ui::menus::omnibox_menu(),
        Some(MenuKind::Link) => mithu_ui::menus::link_menu(),
        Some(MenuKind::Image) => mithu_ui::menus::image_menu(),
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

fn write_page(document: &mithu_ui::pages::Document) -> Url {
    let dir = mithu_ui::pages::directory().join("pages");
    let _ = std::fs::create_dir_all(&dir);
    let name = document.url.trim_start_matches("mithu://").replace('/', "-");
    let path = dir.join(format!("{name}.html"));
    let _ = std::fs::write(&path, &document.html);
    Url::from_file_path(&path).unwrap_or_else(|_| Url::parse("about:blank").expect("blank"))
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

impl Drop for Browser {
    fn drop(&mut self) {
        // Close every webview before Servo asks the constellation to exit.
        // In multiprocess mode that shutdown waits until each content process
        // has been told to stop, and Servo's own drop spins until it finishes.
        self.tabs.clear();
        self.delegate.opened.borrow_mut().clear();
    }
}

fn mithu_config_dir() -> PathBuf {
    #[cfg(windows)]
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return PathBuf::from(appdata).join("mithu");
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("mithu");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".config").join("mithu");
        }
    }
    std::env::temp_dir().join("mithu")
}

impl Browser {
    /// Hide and throttle every tab except the focused one.
    ///
    /// Servo treats `about:blank` as its own site, so loading it in a
    /// background tab starts another content process and does not free the
    /// one that was already running. Hiding is the freeze step Servo
    /// implements: timers and animation frames are throttled.
    fn sync_visibility(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let focused = !self.omnibox_focused && !self.find_focused && self.overlay.is_none();
        let stamp = (self.active, self.tabs.len(), focused);
        if self.visibility_stamp == Some(stamp) {
            return;
        }
        let switched = self.visibility_stamp.map(|old| old.0) != Some(self.active);
        self.visibility_stamp = Some(stamp);
        self.switch_capture |= switched;
        self.scroller.cancel();
        self.restore_discarded_active();
        // Hidden tabs were not painted; draw the newly focused one now.
        self.paint_pending = true;
        self.last_paint = Instant::now() - Self::FRAME_INTERVAL;
        let active = self.active;
        for (index, tab) in self.tabs.iter().enumerate() {
            if index == active {
                tab.webview.show();
                tab.webview.set_focused(focused);
            } else {
                tab.webview.hide();
                tab.webview.set_focused(false);
            }
        }
    }
}

fn strip_servo_token(agent: &str) -> String {
    agent
        .split_whitespace()
        .filter(|token| !token.starts_with("Servo/"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_drops_the_engine_token() {
        let raw = "Mozilla/5.0 (X11; Linux x86_64; rv:153.0) Servo/0.0.1 Firefox/153.0";
        assert_eq!(
            strip_servo_token(raw),
            "Mozilla/5.0 (X11; Linux x86_64; rv:153.0) Firefox/153.0"
        );
    }

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
