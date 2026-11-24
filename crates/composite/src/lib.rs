#![allow(unsafe_op_in_unsafe_fn)]

//! Draw the browser frame over a page that Servo already composited.
//!
//! The page lives in an offscreen framebuffer. This crate uploads the frame
//! Mithu painted with tiny-skia (straight RGBA, transparent where the page
//! should show through) and draws it as one textured quad. Callers keep the
//! OpenGL context current on this thread.

use glow::HasContext;

pub struct ChromePainter {
    program: glow::Program,
    vao: glow::VertexArray,
    #[allow(dead_code)]
    vbo: glow::Buffer,
    texture: glow::Texture,
    size: (i32, i32),
    uploaded: bool,
}

impl ChromePainter {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        // Safety: `gl` is current on this thread for the whole call.
        unsafe { Self::create(gl) }
    }

    pub fn clear_white(gl: &glow::Context, width: i32, height: i32) {
        // Safety: context is current; the viewport covers this window.
        unsafe {
            gl.viewport(0, 0, width.max(1), height.max(1));
            gl.disable(glow::SCISSOR_TEST);
            gl.clear_color(1.0, 1.0, 1.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    /// Draw `rgba` over the current framebuffer. `upload` is false when the
    /// chrome pixels have not changed since the previous present.
    pub fn draw(&mut self, gl: &glow::Context, rgba: &[u8], width: i32, height: i32, upload: bool) {
        if width <= 0 || height <= 0 {
            return;
        }
        let bytes = (width as usize) * (height as usize) * 4;
        let pixels = if upload {
            if rgba.len() < bytes {
                return;
            }
            Some(&rgba[..bytes])
        } else if self.uploaded {
            None
        } else {
            return;
        };
        // Safety: context is current. When `pixels` is set it is a tightly
        // packed RGBA8 image of `width` by `height`.
        unsafe {
            self.draw_gl(gl, pixels, width, height, upload);
        }
    }

    unsafe fn create(gl: &glow::Context) -> Result<Self, String> {
        let version = gl.get_parameter_string(glow::VERSION);
        let es = version.contains("OpenGL ES");
        let (vs_src, fs_src) = if es {
            (VS_ES, FS_ES)
        } else {
            (VS_GL, FS_GL)
        };
        let program = link(gl, vs_src, fs_src)?;
        let vao = gl.create_vertex_array().map_err(|error| error)?;
        let vbo = gl.create_buffer().map_err(|error| error)?;
        let texture = gl.create_texture().map_err(|error| error)?;

        // Full-window triangle strip. UV (0, 0) is the top of the CPU buffer.
        let verts: [f32; 16] = [
            -1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0,
        ];
        let mut bytes = Vec::with_capacity(verts.len() * 4);
        for value in verts {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &bytes, glow::STATIC_DRAW);
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 16, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 16, 8);
        gl.bind_vertex_array(None);

        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);

        if let Some(location) = gl.get_uniform_location(program, "u_tex") {
            gl.use_program(Some(program));
            gl.uniform_1_i32(Some(&location), 0);
            gl.use_program(None);
        }

        Ok(Self {
            program,
            vao,
            vbo,
            texture,
            size: (0, 0),
            uploaded: false,
        })
    }

    unsafe fn draw_gl(&mut self, gl: &glow::Context, rgba: Option<&[u8]>, width: i32, height: i32, upload: bool) {
        gl.viewport(0, 0, width, height);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.enable(glow::BLEND);
        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        if let Some(rgba) = rgba.filter(|_| upload || !self.uploaded || self.size != (width, height)) {
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width,
                height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(rgba)),
            );
            self.size = (width, height);
            self.uploaded = true;
        }
        gl.use_program(Some(self.program));
        gl.bind_vertex_array(Some(self.vao));
        gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        gl.bind_vertex_array(None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.use_program(None);
        gl.disable(glow::BLEND);
    }
}

pub fn renderer_name(gl: &glow::Context) -> String {
    // Safety: context is current.
    unsafe { gl.get_parameter_string(glow::RENDERER) }
}

/// Mesa's software rasterizers and the other known CPU GL implementations.
/// A hardware driver (including a passed-through GPU) returns true.
pub fn is_hardware_renderer(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    const SOFTWARE: &[&str] = &[
        "llvmpipe",
        "softpipe",
        "swrast",
        "swiftshader",
        "microsoft basic render",
        "virgl",
        "lavapipe",
    ];
    !SOFTWARE.iter().any(|needle| name.contains(needle))
}

/// 0RGB chrome pixels (0 means "not painted") become straight RGBA.
pub fn chrome_rgba(pixels: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() * 4);
    for pixel in pixels {
        if *pixel == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            out.push(((*pixel >> 16) & 255) as u8);
            out.push(((*pixel >> 8) & 255) as u8);
            out.push((*pixel & 255) as u8);
            out.push(255);
        }
    }
    out
}

/// How long to wait between paints. Loading pages stay at 10 Hz so raster
/// does not steal the CPU from parsing. A page whose sampled pixels stopped
/// changing backs off, up to about four times a second, until the user
/// scrolls or types.
pub fn paint_interval(loading: bool, stable_level: u32, interacting: bool) -> std::time::Duration {
    use std::time::Duration;
    if loading {
        return Duration::from_millis(100);
    }
    if interacting || stable_level == 0 {
        return Duration::from_micros(16_667);
    }
    let shift = stable_level.min(4);
    Duration::from_micros(16_667u64.saturating_mul(1u64 << shift))
}

/// True when the sampled pixels are almost all the same color, which is what
/// a screenshot callback returns before the page has painted.
pub fn rgba_is_blank(raw: &[u8]) -> bool {
    let pixels = raw.len() / 4;
    if pixels == 0 {
        return true;
    }
    let step = (pixels / 48).max(1);
    let mut seen = 0u32;
    let mut same = 0u32;
    let mut first = [0u8; 4];
    let mut index = 0;
    while index < pixels {
        let offset = index * 4;
        let pixel = &raw[offset..offset + 4];
        if seen == 0 {
            first.copy_from_slice(pixel);
        }
        seen += 1;
        if pixel == first {
            same += 1;
        }
        index += step;
    }
    seen > 0 && same * 100 / seen >= 99
}

unsafe fn link(gl: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    let program = gl.create_program().map_err(|error| error)?;
    let vs = compile(gl, glow::VERTEX_SHADER, vertex)?;
    let fs = compile(gl, glow::FRAGMENT_SHADER, fragment)?;
    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);
    gl.delete_shader(vs);
    gl.delete_shader(fs);
    if !gl.get_program_link_status(program) {
        return Err(gl.get_program_info_log(program));
    }
    Ok(program)
}

unsafe fn compile(gl: &glow::Context, kind: u32, source: &str) -> Result<glow::Shader, String> {
    let shader = gl.create_shader(kind).map_err(|error| error)?;
    gl.shader_source(shader, source);
    gl.compile_shader(shader);
    if !gl.get_shader_compile_status(shader) {
        let log = gl.get_shader_info_log(shader);
        gl.delete_shader(shader);
        return Err(log);
    }
    Ok(shader)
}

const VS_GL: &str = r#"
#version 150
in vec2 a_pos;
in vec2 a_uv;
out vec2 v_uv;
void main() {
    v_uv = a_uv;
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
"#;

const FS_GL: &str = r#"
#version 150
uniform sampler2D u_tex;
in vec2 v_uv;
out vec4 o_color;
void main() {
    o_color = texture(u_tex, v_uv);
}
"#;

const VS_ES: &str = r#"
#version 300 es
in vec2 a_pos;
in vec2 a_uv;
out vec2 v_uv;
void main() {
    v_uv = a_uv;
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
"#;

const FS_ES: &str = r#"
#version 300 es
precision mediump float;
uniform sampler2D u_tex;
in vec2 v_uv;
out vec4 o_color;
void main() {
    o_color = texture(u_tex, v_uv);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_zero_becomes_alpha() {
        let rgba = chrome_rgba(&[0, 0x112233]);
        assert_eq!(&rgba[..4], &[0, 0, 0, 0]);
        assert_eq!(&rgba[4..], &[0x11, 0x22, 0x33, 255]);
    }

    #[test]
    fn loading_paints_slower_than_an_animation() {
        let frame = paint_interval(false, 0, true);
        let loading = paint_interval(true, 0, false);
        let idle = paint_interval(false, 4, false);
        assert!(loading > frame);
        assert!(idle > frame);
        assert!(idle > loading);
    }

    #[test]
    fn a_flat_image_is_blank_and_a_marked_one_is_not() {
        assert!(rgba_is_blank(&[255, 255, 255, 255].repeat(64)));
        let mut raw = vec![255, 255, 255, 255].repeat(64);
        raw[0] = 0;
        raw[1] = 0;
        raw[2] = 0;
        assert!(!rgba_is_blank(&raw));
    }

    #[test]
    fn software_renderers_are_not_hardware() {
        assert!(!is_hardware_renderer("llvmpipe (LLVM 20.1.2, 256 bits)"));
        assert!(is_hardware_renderer("AMD Radeon RX 6800"));
    }
}
