//! Native window for the Mithu browser.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use mithu_shell::{Browser, FrameHit, KeyKind};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

fn app_icon() -> Option<winit::window::Icon> {
    let image = image::load_from_memory(include_bytes!("../../../assets/brand/png/mark-128.png")).ok()?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    winit::window::Icon::from_rgba(rgba.into_raw(), width, height).ok()
}

fn main() {
    mithu_host::init("mithu");
    // Windows rejects a winit event loop that is not on the main thread, and
    // the process then exits as soon as the console appears. The other
    // platforms keep a 32MB worker because their default main stack is small
    // and winit allows the loop off-thread. The Windows stack reserve is set
    // in build.rs.
    #[cfg(windows)]
    run();
    #[cfg(not(windows))]
    {
        let worker = std::thread::Builder::new()
            .name("mithu".into())
            .stack_size(32 * 1024 * 1024)
            .spawn(run)
            .expect("start mithu");
        if let Err(payload) = worker.join() {
            std::panic::resume_unwind(payload);
        }
    }
}

fn run() {
    let mut args = std::env::args().skip(1);
    if let Some(first) = args.next() {
        if first == "--version" || first == "-V" {
            println!("mithu {}", env!("MITHU_VERSION"));
            return;
        }
        if first == "--hold" {
            let seconds = args
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(10);
            let url = args.next().unwrap_or_else(|| "about:blank".to_string());
            let tabs = args
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1);
            hold(seconds, &url, tabs);
            return;
        }
        if first == "--smoke-test" {
            let url = args.next().unwrap_or_else(|| "about:blank".to_string());
            if let Err(error) = smoke(&url) {
                report_fatal(&error);
            }
            return;
        }
        if first == "--screenshot" {
            let path = args.next().unwrap_or_else(|| "mithu.png".to_string());
            let url = args.next();
            if let Err(error) = dump_screenshot(&path, url.as_deref()) {
                eprintln!("mithu: {error}");
                std::process::exit(1);
            }
            return;
        }
        let event_loop = event_loop();
        let mut app = App::new();
        app.browser.navigate(&first);
        if let Err(error) = event_loop.run_app(&mut app) {
            report_fatal(&format!("could not run the window: {error}"));
        }
        return;
    }
    let event_loop = event_loop();
    let mut app = App::new();
    if let Err(error) = event_loop.run_app(&mut app) {
        report_fatal(&format!("could not run the window: {error}"));
    }
}

fn smoke(url: &str) -> Result<(), String> {
    mithu_host::suppress_dialog();
    let mut browser = mithu_shell::Browser::try_new()?;
    browser.resize(800.0, 600.0);
    if url != "about:blank" {
        browser.navigate(url);
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(20) && !browser.load_settled() {
            browser.pump_scripts(Duration::from_millis(50));
        }
    }
    let frame = browser.render(800.0, 600.0);
    if frame.iter().all(|pixel| *pixel == 0) {
        return Err("smoke test painted an empty frame".into());
    }
    println!("smoke ok");
    Ok(())
}

fn hold(seconds: u64, url: &str, tabs: usize) {
    let mut browser = Browser::new();
    browser.resize(1280.0, 800.0);
    let started = Instant::now();
    browser.navigate(url);
    for _ in 1..tabs.max(1) {
        browser.new_tab();
        browser.navigate(url);
    }
    while started.elapsed() < Duration::from_secs(45) && !browser.load_settled() {
        browser.pump_scripts(Duration::from_millis(100));
    }
    let _ = browser.render(1280.0, 800.0);
    println!("loaded_ms {}", started.elapsed().as_millis());
    std::thread::sleep(Duration::from_secs(seconds));
}

fn event_loop() -> EventLoop<()> {
    let mut builder = EventLoop::builder();
    #[cfg(target_os = "linux")]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_any_thread(true);
    }
    match builder.build() {
        Ok(loop_) => loop_,
        Err(error) => report_fatal(&format!("could not create the event loop: {error}")),
    }
}

fn dump_screenshot(path: &str, url: Option<&str>) -> Result<(), String> {
    let mut browser = Browser::new();
    if let Some(url) = url {
        // Parser-blocking work stays inside navigate. The rest of the scripts
        // run here, sharing one 15 second budget with timers, so a page with
        // dozens of bundles still paints.
        browser.set_cooperative_scripts(true);
        let budget = Instant::now();
        let nav = Instant::now();
        browser.navigate(url);
        if std::env::var_os("MITHU_TIMING").is_some() {
            eprintln!("timing navigate {}ms", nav.elapsed().as_millis());
        }
        let started = Instant::now();
        let mut seen = browser.content_revision();
        let mut stable_since = Instant::now();
        // Servo finishes the load on its own threads. Keep spinning until the
        // load settles, then until the frame has been quiet for 800ms.
        while started.elapsed() < Duration::from_secs(20)
            && budget.elapsed() < Duration::from_secs(25)
        {
            let pending = browser.pump_scripts(Duration::from_millis(200));
            let revision = browser.content_revision();
            if revision != seen {
                seen = revision;
                stable_since = Instant::now();
            }
            if pending {
                continue;
            }
            browser.advance_timers(0);
            let frames = browser.animation_frames_pending();
            let quiet = stable_since.elapsed() > Duration::from_millis(800)
                && started.elapsed() > Duration::from_secs(1);
            // Keep scheduling animation frames while a timeline (GSAP fromTo)
            // is still queued. A ticker that never clears still stops after a
            // few seconds so the shot stays inside its budget.
            if quiet && !frames {
                break;
            }
            if started.elapsed() > Duration::from_millis(3500) {
                break;
            }
            if frames {
                continue;
            }
            let Some(wait) = browser.next_timer_ms() else {
                break;
            };
            if wait > 1_000 {
                break;
            }
            browser.advance_timers(wait.clamp(1, 250));
        }
        eprintln!("load_ms {}", budget.elapsed().as_millis());
    }
    let width = 1280u32;
    let height = 800u32;
    let paint = Instant::now();
    let pixels = browser.render(width as f32, height as f32);
    if std::env::var_os("MITHU_TIMING").is_some() {
        eprintln!("timing paint {}ms", paint.elapsed().as_millis());
    }
    let mut rgb = Vec::with_capacity(pixels.len() * 3);
    for pixel in pixels {
        rgb.push(((pixel >> 16) & 255) as u8);
        rgb.push(((pixel >> 8) & 255) as u8);
        rgb.push((pixel & 255) as u8);
    }
    let png = encode_png(width, height, &rgb)?;
    std::fs::write(path, png).map_err(|error| error.to_string())?;
    if !browser.status_line().is_empty() {
        eprintln!("{}", browser.status_line());
    }
    if std::env::var("MITHU_LOGS").is_ok() {
        for line in browser.script_logs() {
            eprintln!("LOG {line}");
        }
    }
    Ok(())
}

fn encode_png(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    image::ImageEncoder::write_image(encoder, rgb, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

struct App {
    browser: Browser,
    window: Option<Arc<Window>>,
    context: Option<softbuffer::Context<Arc<Window>>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    modifiers: ModifiersState,
    cursor: (f32, f32),
    timer_mark: Instant,
}

impl App {
    fn new() -> Self {
        let mut browser = Browser::new();
        browser.set_cooperative_scripts(true);
        browser.follow_os_theme();
        Self {
            browser,
            window: None,
            context: None,
            surface: None,
            modifiers: ModifiersState::default(),
            cursor: (0.0, 0.0),
            timer_mark: Instant::now(),
        }
    }

    fn forward_key(&mut self, key: &Key, pressed: bool) {
        if self.browser.omnibox_focused || self.browser.find_focused {
            return;
        }
        let kind = match key {
            Key::Named(NamedKey::Enter) => KeyKind::Enter,
            Key::Named(NamedKey::Escape) => KeyKind::Escape,
            Key::Named(NamedKey::Backspace) => KeyKind::Backspace,
            Key::Named(NamedKey::Tab) => KeyKind::Tab,
            Key::Named(NamedKey::ArrowLeft) => KeyKind::Left,
            Key::Named(NamedKey::ArrowRight) => KeyKind::Right,
            Key::Named(NamedKey::ArrowUp) => KeyKind::Up,
            Key::Named(NamedKey::ArrowDown) => KeyKind::Down,
            Key::Named(NamedKey::PageUp) => KeyKind::PageUp,
            Key::Named(NamedKey::PageDown) => KeyKind::PageDown,
            Key::Named(NamedKey::Home) => KeyKind::Home,
            Key::Named(NamedKey::End) => KeyKind::End,
            Key::Named(NamedKey::Space) => KeyKind::Space,
            Key::Character(text) => {
                for ch in text.chars().take(1) {
                    self.browser.send_key(
                        KeyKind::Char(ch),
                        pressed,
                        self.modifiers.control_key(),
                        self.modifiers.alt_key(),
                        self.modifiers.shift_key(),
                    );
                }
                return;
            }
            _ => return,
        };
        self.browser.send_key(
            kind,
            pressed,
            self.modifiers.control_key(),
            self.modifiers.alt_key(),
            self.modifiers.shift_key(),
        );
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn sync_title(&self) {
        if let Some(window) = &self.window {
            let title = if self.browser.title().is_empty() {
                format!("Mithu {}", env!("MITHU_VERSION"))
            } else {
                format!("{} — Mithu {}", self.browser.title(), env!("MITHU_VERSION"))
            };
            window.set_title(&title);
        }
    }

    fn redraw(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
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
        let frame = self.browser.render(px_w as f32, px_h as f32);
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        let count = buffer.len().min(frame.len());
        buffer[..count].copy_from_slice(&frame[..count]);
        let _ = buffer.present();
        self.sync_title();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(format!("Mithu {}", env!("MITHU_VERSION")))
            .with_window_icon(app_icon())
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(1200.0, 800.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => report_fatal(&format!("could not open a window: {error}")),
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(context) => context,
            Err(error) => report_fatal(&format!("could not create a framebuffer: {error}")),
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => report_fatal(&format!("could not create a surface: {error}")),
        };
        let size = window.inner_size();
        self.browser.set_scale(window.scale_factor() as f32);
        self.browser
            .resize(size.width.max(1) as f32, size.height.max(1) as f32);
        self.context = Some(context);
        self.surface = Some(surface);
        self.window = Some(window);
        self.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(window) = &self.window {
                    self.browser.set_scale(window.scale_factor() as f32);
                }
                self.browser
                    .resize(size.width.max(1) as f32, size.height.max(1) as f32);
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                self.browser.pointer_move(self.cursor.0, self.cursor.1);
                self.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed && button == MouseButton::Right {
                    self.browser.context_menu(self.cursor.0, self.cursor.1);
                    self.sync_title();
                    self.request_redraw();
                } else if button == MouseButton::Left && state == ElementState::Pressed {
                    match self.browser.frame_hit(self.cursor.0, self.cursor.1) {
                        Some(FrameHit::Close) => event_loop.exit(),
                        Some(FrameHit::Minimize) => {
                            if let Some(window) = &self.window {
                                let _ = window.set_minimized(true);
                            }
                        }
                        Some(FrameHit::Maximize) => {
                            if let Some(window) = &self.window {
                                window.set_maximized(!window.is_maximized());
                            }
                        }
                        Some(FrameHit::Drag) => {
                            if let Some(window) = &self.window {
                                let _ = window.drag_window();
                            }
                        }
                        None => self.browser.pointer_down(self.cursor.0, self.cursor.1),
                    }
                    self.sync_title();
                    self.request_redraw();
                } else if button == MouseButton::Left && state == ElementState::Released {
                    self.browser.pointer_up();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 64.0,
                    MouseScrollDelta::PixelDelta(position) => -position.y as f32,
                };
                self.browser.scroll_by(dy);
                self.request_redraw();
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.browser.ime_commit(&text);
                self.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    self.forward_key(&event.logical_key, false);
                    return;
                }
                let ctrl = self.modifiers.control_key();
                let alt = self.modifiers.alt_key();
                let shift = self.modifiers.shift_key();
                let focused = self.browser.omnibox_focused;
                match event.logical_key {
                    Key::Named(NamedKey::Enter) if self.browser.find_focused && shift => {
                        self.browser.find_previous()
                    }
                    Key::Named(NamedKey::Enter) if self.browser.find_focused => {
                        self.browser.find_next()
                    }
                    Key::Named(NamedKey::Enter) if focused => self.browser.submit_omnibox(),
                    Key::Named(NamedKey::Enter) if self.browser.field_focused() => {
                        self.browser.submit_field()
                    }
                    Key::Named(NamedKey::Escape) if self.browser.menu_open() => {
                        self.browser.close_menu()
                    }
                    Key::Named(NamedKey::Escape) if self.browser.bookmarks_open() => {
                        self.browser.close_bookmarks()
                    }
                    Key::Named(NamedKey::Escape) if self.browser.find_is_open() => {
                        self.browser.close_find()
                    }
                    Key::Named(NamedKey::Escape) => self.browser.blur_page(),
                    Key::Named(NamedKey::Backspace) if self.browser.find_focused => {
                        self.browser.find_backspace()
                    }
                    Key::Named(NamedKey::Backspace) if focused => self.browser.backspace(),
                    Key::Named(NamedKey::Backspace) if self.browser.field_focused() => {
                        self.browser.field_backspace()
                    }
                    Key::Named(NamedKey::ArrowDown) if focused => self.browser.move_suggestion(1),
                    Key::Named(NamedKey::ArrowUp) if focused => self.browser.move_suggestion(-1),
                    Key::Named(NamedKey::ArrowLeft) if focused && !alt => {
                        self.browser.move_cursor(-1)
                    }
                    Key::Named(NamedKey::ArrowRight) if focused && !alt => {
                        self.browser.move_cursor(1)
                    }
                    Key::Named(NamedKey::ArrowLeft) if alt => self.browser.back(),
                    Key::Named(NamedKey::ArrowRight) if alt => self.browser.forward(),
                    Key::Named(NamedKey::ArrowDown) if !focused => self.browser.scroll_by(64.0),
                    Key::Named(NamedKey::ArrowUp) if !focused => self.browser.scroll_by(-64.0),
                    Key::Named(NamedKey::PageDown) if !focused => self.browser.scroll_page(1.0),
                    Key::Named(NamedKey::PageUp) if !focused => self.browser.scroll_page(-1.0),
                    Key::Named(NamedKey::Home) if !focused => self.browser.scroll_by(-1.0e9),
                    Key::Named(NamedKey::End) if !focused => self.browser.scroll_by(1.0e9),
                    Key::Named(NamedKey::Tab) if ctrl && shift => self.browser.previous_tab(),
                    Key::Named(NamedKey::Tab) if ctrl => self.browser.next_tab(),
                    Key::Named(NamedKey::F5) => self.browser.reload(),
                    Key::Character(text) if ctrl && shift && text.eq_ignore_ascii_case("t") => {
                        self.browser.reopen_closed_tab()
                    }
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("t") => {
                        self.browser.new_tab()
                    }
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("n") => {
                        self.browser.new_tab()
                    }
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("h") => {
                        self.browser.navigate("about:history")
                    }
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("j") => {
                        self.browser.navigate("about:downloads")
                    }
                    Key::Character(text) if ctrl && text == "," => {
                        self.browser.navigate("about:settings")
                    }
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("w") => {
                        self.browser.close_tab()
                    }
                    Key::Character(text) if ctrl && (text == "+" || text == "=") => {
                        self.browser.zoom_in()
                    }
                    Key::Character(text) if ctrl && (text == "-" || text == "_") => {
                        self.browser.zoom_out()
                    }
                    Key::Character(text) if ctrl && text == "0" => self.browser.zoom_reset(),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("d") => {
                        self.browser.toggle_bookmark()
                    }
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("b") => {
                        self.browser.toggle_bookmark_list()
                    }
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("f") => {
                        self.browser.open_find()
                    }
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("l") => {
                        self.browser.focus_omnibox()
                    }
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("r") => {
                        self.browser.reload()
                    }
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("a") && focused => {
                        self.browser.select_all_text();
                    }
                    Key::Character(text) if self.browser.find_focused && !ctrl && !alt => {
                        for ch in text.chars() {
                            self.browser.find_insert(ch);
                        }
                    }
                    Key::Character(text) if focused && !ctrl && !alt => {
                        for ch in text.chars() {
                            self.browser.insert_char(ch);
                        }
                    }
                    Key::Character(text) if self.browser.field_focused() && !ctrl && !alt => {
                        for ch in text.chars() {
                            self.browser.field_insert(ch);
                        }
                    }
                    Key::Named(NamedKey::Space) if self.browser.field_focused() => {
                        self.browser.field_insert(' ')
                    }
                    Key::Named(NamedKey::Space) if !focused => self.browser.scroll_page(1.0),
                    _ => self.forward_key(&event.logical_key, true),
                }
                self.sync_title();
                self.request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let elapsed = self.timer_mark.elapsed().as_millis() as u64;
        self.timer_mark = Instant::now();
        let scripts =
            self.browser.pump_scripts(Duration::from_millis(12)) || self.browser.chrome_animating();
        // Drive frames while scripts are still loading. A fromTo that starts
        // in an effect would otherwise sit at opacity 0 until the bundles end.
        if self.browser.animation_frames_pending() {
            let step = elapsed.clamp(1, 34);
            if self.browser.advance_frame(step) {
                self.sync_title();
                self.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(16),
            ));
            return;
        }
        if scripts {
            if elapsed > 0 {
                self.browser.advance_limited(elapsed.clamp(1, 34), None, 2);
            }
            self.sync_title();
            self.request_redraw();
            event_loop.set_control_flow(ControlFlow::Poll);
            return;
        }
        if elapsed > 0 && self.browser.advance_timers(elapsed) {
            self.sync_title();
            self.request_redraw();
        }
        let wait = self
            .browser
            .next_timer_ms()
            .unwrap_or(60_000)
            .clamp(1, 60_000);
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(wait),
        ));
    }
}

fn report_fatal(message: &str) -> ! {
    mithu_host::report_fatal(message);
}

#[cfg(test)]
mod version_source {
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
}
