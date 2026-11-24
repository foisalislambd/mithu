//! Window compositor for the Servo edition.
//!
//! Servo paints the page into an offscreen framebuffer. When a window GL
//! context exists, that framebuffer is blitted into the content rectangle and
//! the browser frame (tiny-skia / resvg / fontdue) is drawn over it. The
//! window then swaps buffers. Nothing reads the page pixels back to the CPU.
//!
//! If the window context cannot be created, the caller keeps the OSMesa
//! software framebuffer and copies it into softbuffer.

use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use euclid::default::{Point2D, Rect, Size2D};
use raw_window_handle::{DisplayHandle, WindowHandle};
use servo::{OffscreenRenderingContext, RenderingContext, WindowRenderingContext};

use mithu_composite::ChromePainter;

pub struct Compositor {
    window: Rc<WindowRenderingContext>,
    page: Rc<OffscreenRenderingContext>,
    gl: Arc<glow::Context>,
    painter: ChromePainter,
    renderer: String,
    hardware: bool,
}

impl Compositor {
    pub fn attach(
        display: DisplayHandle<'_>,
        window: WindowHandle<'_>,
        size: PhysicalSize<u32>,
    ) -> Result<Self, String> {
        let size = PhysicalSize::new(size.width.max(1), size.height.max(1));
        let window = WindowRenderingContext::new(display, window, size)
            .map_err(|error| format!("window GL context: {error:?}"))?;
        let window = Rc::new(window);
        window
            .make_current()
            .map_err(|error| format!("make current: {error:?}"))?;
        let gl = window.glow_gl_api();
        let renderer = mithu_composite::renderer_name(&gl);
        let hardware = mithu_composite::is_hardware_renderer(&renderer);
        let painter = ChromePainter::new(&gl)?;
        let page = Rc::new(window.offscreen_context(size));
        let kind = if hardware { "hardware" } else { "software GL" };
        eprintln!("mithu: compositing on {renderer} ({kind})");
        Ok(Self {
            window,
            page,
            gl,
            painter,
            renderer,
            hardware,
        })
    }

    pub fn renderer(&self) -> &str {
        &self.renderer
    }

    pub fn hardware(&self) -> bool {
        self.hardware
    }

    pub fn size(&self) -> (u32, u32) {
        let size = self.window.size();
        (size.width, size.height)
    }

    pub fn page_context(&self) -> Rc<dyn RenderingContext> {
        self.page.clone()
    }

    pub fn resize(&self, window: PhysicalSize<u32>, page: PhysicalSize<u32>) {
        if window.width > 0 && window.height > 0 {
            self.window.resize(window);
        }
        if page.width > 0 && page.height > 0 && self.page.size() != page {
            self.page.resize(page);
        }
    }

    /// Blit the page into the content rectangle and draw the chrome texture.
    /// `content_top` is the chrome height in physical pixels. `upload` replaces
    /// the chrome texture; a page-only frame reuses the one already on the GPU.
    pub fn present(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        content_top: i32,
        blit_page: bool,
        upload: bool,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let _ = self.window.make_current();
        self.window.prepare_for_rendering();
        let win_w = width as i32;
        let win_h = height as i32;
        ChromePainter::clear_white(&self.gl, win_w, win_h);
        if blit_page {
            if let Some(callback) = self.page.render_to_parent_callback() {
                let top = content_top.clamp(0, win_h.saturating_sub(1));
                let content_h = (win_h - top).max(1);
                let from_bottom = win_h - top - content_h;
                let rect = Rect::new(Point2D::new(0, from_bottom), Size2D::new(win_w, content_h));
                callback(&self.gl, rect);
            }
        }
        self.window.prepare_for_rendering();
        self.painter.draw(&self.gl, rgba, win_w, win_h, upload);
        self.window.present();
    }
}
