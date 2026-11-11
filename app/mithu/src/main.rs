//! Native window for the Mithu browser.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mithu_shell::{Browser, FrameHit};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

fn main() {
    let run = std::thread::Builder::new()
        .name("mithu".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(run)
        .expect("start mithu");
    if let Err(payload) = run.join() {
        std::panic::resume_unwind(payload);
    }
}

fn run() {
    let mut args = std::env::args().skip(1);
    if let Some(first) = args.next() {
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
        event_loop.run_app(&mut app).expect("run the window");
        return;
    }
    let event_loop = event_loop();
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("run the window");
}

fn event_loop() -> EventLoop<()> {
    let mut builder = EventLoop::builder();
    #[cfg(target_os = "linux")]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_any_thread(true);
    }
    builder.build().expect("create the event loop")
}

fn dump_screenshot(path: &str, url: Option<&str>) -> Result<(), String> {
    let mut browser = Browser::new();
    if let Some(url) = url {
        browser.navigate(url);
        // Let a short timer (Google's own fallback, for example) run before
        // the still is taken.
        browser.advance_timers(2_500);
    }
    let width = 1280u32;
    let height = 800u32;
    let pixels = browser.render(width as f32, height as f32);
    let mut rgb = Vec::with_capacity(pixels.len() * 3);
    for pixel in pixels {
        rgb.push(((pixel >> 16) & 255) as u8);
        rgb.push(((pixel >> 8) & 255) as u8);
        rgb.push((pixel & 255) as u8);
    }
    let png = mithu_image::encode_png(width, height, &rgb)
        .ok_or_else(|| "could not encode the screenshot".to_string())?;
    std::fs::write(path, png).map_err(|error| error.to_string())?;
    if !browser.status_line().is_empty() {
        eprintln!("{}", browser.status_line());
    }
    Ok(())
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

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn sync_title(&self) {
        if let Some(window) = &self.window {
            let title = if self.browser.title().is_empty() {
                "Mithu".to_string()
            } else {
                format!("{} — Mithu", self.browser.title())
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
        let frame = self.browser.render(width.get() as f32, height.get() as f32);
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
            .with_title("Mithu")
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(1200.0, 800.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("mithu: could not open a window: {error}");
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("mithu: could not create a framebuffer: {error}");
                event_loop.exit();
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => {
                eprintln!("mithu: could not create a surface: {error}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
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
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
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
                    _ => return,
                }
                self.sync_title();
                self.request_redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.browser.pump_scripts(Duration::from_millis(12)) || self.browser.chrome_animating() {
            self.sync_title();
            self.request_redraw();
            event_loop.set_control_flow(ControlFlow::Poll);
            return;
        }
        let elapsed = self.timer_mark.elapsed().as_millis() as u64;
        self.timer_mark = Instant::now();
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
