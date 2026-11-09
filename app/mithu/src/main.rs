//! Native window for the Mithu browser.

use std::sync::Arc;

use mithu_shell::Browser;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

fn main() {
    let event_loop = EventLoop::new().expect("create the event loop");
    let mut app = App::new();
    if let Some(argument) = std::env::args().nth(1) {
        app.browser.navigate(&argument);
    }
    event_loop.run_app(&mut app).expect("run the window");
}

struct App {
    browser: Browser,
    window: Option<Arc<Window>>,
    context: Option<softbuffer::Context<Arc<Window>>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    modifiers: ModifiersState,
    cursor: (f32, f32),
}

impl App {
    fn new() -> Self {
        Self {
            browser: Browser::new(),
            window: None,
            context: None,
            surface: None,
            modifiers: ModifiersState::default(),
            cursor: (0.0, 0.0),
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
        self.browser.resize(size.width.max(1) as f32, size.height.max(1) as f32);
        self.context = Some(context);
        self.surface = Some(surface);
        self.window = Some(window);
        self.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.browser.resize(size.width.max(1) as f32, size.height.max(1) as f32);
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left && state == ElementState::Pressed {
                    self.browser.pointer_down(self.cursor.0, self.cursor.1);
                    self.sync_title();
                    self.request_redraw();
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
                    Key::Named(NamedKey::Enter) if self.browser.find_focused && shift => self.browser.find_previous(),
                    Key::Named(NamedKey::Enter) if self.browser.find_focused => self.browser.find_next(),
                    Key::Named(NamedKey::Enter) if focused => self.browser.submit_omnibox(),
                    Key::Named(NamedKey::Enter) if self.browser.field_focused() => self.browser.submit_field(),
                    Key::Named(NamedKey::Escape) if self.browser.find_is_open() => self.browser.close_find(),
                    Key::Named(NamedKey::Escape) => self.browser.blur_page(),
                    Key::Named(NamedKey::Backspace) if self.browser.find_focused => self.browser.find_backspace(),
                    Key::Named(NamedKey::Backspace) if focused => self.browser.backspace(),
                    Key::Named(NamedKey::Backspace) if self.browser.field_focused() => self.browser.field_backspace(),
                    Key::Named(NamedKey::ArrowLeft) if focused && !alt => self.browser.move_cursor(-1),
                    Key::Named(NamedKey::ArrowRight) if focused && !alt => self.browser.move_cursor(1),
                    Key::Named(NamedKey::ArrowLeft) if alt => self.browser.back(),
                    Key::Named(NamedKey::ArrowRight) if alt => self.browser.forward(),
                    Key::Named(NamedKey::ArrowDown) if !focused => self.browser.scroll_by(64.0),
                    Key::Named(NamedKey::ArrowUp) if !focused => self.browser.scroll_by(-64.0),
                    Key::Named(NamedKey::PageDown) if !focused => self.browser.scroll_page(1.0),
                    Key::Named(NamedKey::PageUp) if !focused => self.browser.scroll_page(-1.0),
                    Key::Named(NamedKey::Home) if !focused => self.browser.scroll_by(-1.0e9),
                    Key::Named(NamedKey::End) if !focused => self.browser.scroll_by(1.0e9),
                    Key::Named(NamedKey::Tab) if ctrl => self.browser.next_tab(),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("t") => self.browser.new_tab(),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("w") => self.browser.close_tab(),
                    Key::Character(text) if ctrl && (text == "+" || text == "=") => self.browser.zoom_in(),
                    Key::Character(text) if ctrl && (text == "-" || text == "_") => self.browser.zoom_out(),
                    Key::Character(text) if ctrl && text == "0" => self.browser.zoom_reset(),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("f") => self.browser.open_find(),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("l") => self.browser.focus_omnibox(),
                    Key::Character(text) if ctrl && text.eq_ignore_ascii_case("r") => self.browser.reload(),
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
                    Key::Named(NamedKey::Space) if self.browser.field_focused() => self.browser.field_insert(' '),
                    Key::Named(NamedKey::Space) if !focused => self.browser.scroll_page(1.0),
                    _ => return,
                }
                self.sync_title();
                self.request_redraw();
            }
            _ => {}
        }
    }
}
