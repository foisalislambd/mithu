//! Window-thread session.
//!
//! Servo and the window GL context have to live on the same thread: the
//! context cannot be current on two threads, and presenting the page without
//! a CPU readback means painting it here. Input is applied directly and the
//! event loop wakes when Servo posts work.

use mithu_shell::Browser;

pub enum UiAction {
    Exit,
    Minimize,
    Maximize,
    Drag,
}

pub struct Session {
    pub browser: Browser,
    pub modifiers_control: bool,
    pub modifiers_alt: bool,
    pub modifiers_shift: bool,
    pub cursor: (f32, f32),
    pub width: f32,
    pub height: f32,
}

impl Session {
    pub fn new(browser: Browser, width: f32, height: f32) -> Self {
        Self {
            browser,
            modifiers_control: false,
            modifiers_alt: false,
            modifiers_shift: false,
            cursor: (0.0, 0.0),
            width,
            height,
        }
    }
}
