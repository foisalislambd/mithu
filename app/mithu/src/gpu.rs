//! wgpu presentation for a Vello scene. Creation fails when the machine has
//! no compatible adapter, and the window keeps the CPU painter.

use std::num::NonZeroUsize;
use std::sync::Arc;

use vello::wgpu::util::TextureBlitter;
use vello::wgpu::{self, TextureFormat};
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};
use winit::window::Window;

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    blitter: TextureBlitter,
    target: Option<wgpu::Texture>,
    target_view: Option<wgpu::TextureView>,
}

impl Gpu {
    pub fn new(window: Arc<Window>) -> Option<Self> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::all();
        let instance = wgpu::Instance::new(desc);
        let surface = instance.create_surface(window.clone()).ok()?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mithu"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|format| {
                matches!(
                    format,
                    TextureFormat::Rgba8Unorm | TextureFormat::Bgra8Unorm
                )
            })
            .or(caps.formats.first().copied())?;
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            color_space: wgpu::SurfaceColorSpace::Auto,
            alpha_mode: caps.alpha_modes.first().copied()?,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let renderer = Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: NonZeroUsize::new(1),
                pipeline_cache: None,
            },
        )
        .ok()?;
        let blitter = TextureBlitter::new(&device, format);
        Some(Self {
            device,
            queue,
            surface,
            config,
            renderer,
            blitter,
            target: None,
            target_view: None,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
        self.target = None;
        self.target_view = None;
    }

    fn ensure_target(&mut self) {
        let width = self.config.width;
        let height = self.config.height;
        let stale = self.target.as_ref().is_none_or(|texture| {
            texture.width() != width || texture.height() != height
        });
        if stale {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mithu-vello"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            self.target_view = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
            self.target = Some(texture);
        }
    }

    pub fn render(&mut self, scene: &Scene) -> Result<(), String> {
        self.ensure_target();
        let width = self.config.width;
        let height = self.config.height;
        let view = self.target_view.take().ok_or("vello target")?;
        let rendered = self.renderer.render_to_texture(
            &self.device,
            &self.queue,
            scene,
            &view,
            &RenderParams {
                base_color: vello::peniko::Color::from_rgb8(255, 255, 255),
                width,
                height,
                antialiasing_method: AaConfig::Area,
            },
        );
        if let Err(error) = rendered {
            self.target_view = Some(view);
            return Err(error.to_string());
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                self.target_view = Some(view);
                return Ok(());
            }
            other => {
                self.target_view = Some(view);
                return Err(format!("surface frame {other:?}"));
            }
        };
        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mithu-blit"),
            });
        self.blitter
            .copy(&self.device, &mut encoder, &view, &surface_view);
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        self.target_view = Some(view);
        Ok(())
    }
}

/// Headless device used by the paint benchmark. `None` when wgpu finds no adapter.
pub struct Headless {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl Headless {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::all();
        let instance = wgpu::Instance::new(desc);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: true,
            apply_limit_buckets: false,
        }))
        .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mithu-bench"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()?;
        let renderer = Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: NonZeroUsize::new(1),
                pipeline_cache: None,
            },
        )
        .ok()?;
        let (texture, view) = target(&device, width, height);
        Some(Self {
            device,
            queue,
            renderer,
            texture,
            view,
            width,
            height,
        })
    }

    pub fn render(&mut self, scene: &Scene) -> Result<(), String> {
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                scene,
                &self.view,
                &RenderParams {
                    base_color: vello::peniko::Color::from_rgb8(255, 255, 255),
                    width: self.width,
                    height: self.height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|error| error.to_string())?;
        let _ = &self.texture;
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

fn target(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mithu-bench"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}
