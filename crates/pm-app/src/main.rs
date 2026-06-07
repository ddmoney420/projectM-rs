//! `pm-app` — a live window that renders Milkdrop presets reacting to audio.
//!
//! ```text
//! cargo run -p pm-app --release -- [preset-dir]
//! ```
//!
//! Keys: →/Space/N next preset · ←/P previous · R random · Esc/Q quit.

mod audio;
mod blit;

use audio::AudioInput;
use blit::Blit;
use pm_core::WarpEngine;
use pm_preset::Preset;
use pm_render::wgpu;
use pm_render::GpuContext;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

const DEFAULT_DIR: &str = r"C:\My-Workspace\projectm-presets\cream-of-the-crop";

/// A fallback preset used when no preset files are found.
const BUILTIN: &str = "\
fDecay=0.975
zoom=1.012
rot=0.006
warp=0.9
bTexWrap=1
nWaveMode=0
bAdditiveWaves=1
fWaveAlpha=1.0
fWaveScale=2.0
per_frame_1=`wave_r = 0.5 + 0.5*sin(time*1.3);
per_frame_2=`wave_g = 0.5 + 0.5*sin(time*1.7 + 2);
per_frame_3=`wave_b = 0.5 + 0.5*sin(time*2.3 + 4);
per_pixel_1=`rot = rot + 0.15*(rad - 0.5);
";

struct Render {
    window: Arc<Window>,
    ctx: GpuContext,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    engine: WarpEngine,
    blit: Blit,
}

struct App {
    presets: Vec<PathBuf>,
    index: usize,
    rng: u32,
    audio: AudioInput,
    render: Option<Render>,
    start: Instant,
    last: Instant,
    frame: i32,
}

impl App {
    fn new(presets: Vec<PathBuf>) -> Self {
        let now = Instant::now();
        App {
            presets,
            index: 0,
            rng: 0x1234_5678,
            audio: AudioInput::new(),
            render: None,
            start: now,
            last: now,
            frame: 0,
        }
    }

    /// Resolve and load the current preset, falling back to the built-in.
    fn current_preset(&self) -> (Preset, String) {
        if let Some(path) = self.presets.get(self.index) {
            if let Ok(bytes) = std::fs::read(path) {
                let content = String::from_utf8_lossy(&bytes);
                if let Ok(preset) = Preset::load(&content) {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    return (preset, name);
                }
            }
        }
        (Preset::load(BUILTIN).expect("builtin preset"), "built-in".into())
    }

    /// Advance to the next loadable preset in the given direction (or random).
    fn change_preset(&mut self, delta: i64, random: bool) {
        if self.presets.is_empty() {
            return;
        }
        let len = self.presets.len();
        if random {
            self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            self.index = (self.rng as usize) % len;
        } else {
            self.index = ((self.index as i64 + delta).rem_euclid(len as i64)) as usize;
        }
        self.rebuild_engine();
    }

    /// Recreate the warp engine for the current preset at the current size.
    fn rebuild_engine(&mut self) {
        let (preset, name) = self.current_preset();
        if let Some(render) = &mut self.render {
            let (w, h) = (render.config.width, render.config.height);
            render.engine = WarpEngine::new(&render.ctx, preset, w, h);
            let cc = if render.engine.uses_custom_composite() { " ·custom-comp" } else { "" };
            render.window.set_title(&format!(
                "pm-app — {name}{cc}  [{}/{}]",
                self.index + 1,
                self.presets.len().max(1)
            ));
        }
    }

    fn render(&mut self) {
        let Some(render) = &mut self.render else { return };

        let now = Instant::now();
        let dt = (now - self.last).as_secs_f64().min(0.1);
        self.last = now;
        let time = (now - self.start).as_secs_f32();

        let audio = self.audio.frame_data(dt, self.frame);
        let _ = render.engine.render_frame(&render.ctx, time, self.frame, audio);
        self.frame += 1;

        match render.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                render.blit.draw(&render.ctx, render.engine.display_texture(), &view);
                frame.present();
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                render.surface.configure(&render.ctx.device, &render.config);
            }
            // Timeout / Occluded / Validation: skip this frame.
            _ => {}
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if let Some(render) = &mut self.render {
            render.config.width = width;
            render.config.height = height;
            render.surface.configure(&render.ctx.device, &render.config);
        }
        // Recreate the engine to match the new resolution.
        self.rebuild_engine();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.render.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("pm-app")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 720.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone()).expect("create surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .expect("no compatible GPU adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("pm-app device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .expect("request device");

        println!("Rendering on: {}", adapter.get_info().name);
        println!("Audio source: {}", self.audio.source());

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w,
            height: h,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let ctx = GpuContext { instance, adapter, device, queue };
        let blit = Blit::new(&ctx, format);
        let (preset, name) = self.current_preset();
        let engine = WarpEngine::new(&ctx, preset, w, h);
        let cc = if engine.uses_custom_composite() { " ·custom-comp" } else { "" };
        window.set_title(&format!("pm-app — {name}{cc}  [{}/{}]", self.index + 1, self.presets.len().max(1)));

        self.render = Some(Render { window, ctx, surface, config, engine, blit });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resize(size.width, size.height),
            WindowEvent::RedrawRequested => self.render(),
            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key, state: ElementState::Pressed, .. },
                ..
            } => match logical_key {
                Key::Named(NamedKey::Escape) => event_loop.exit(),
                Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::Space) => self.change_preset(1, false),
                Key::Named(NamedKey::ArrowLeft) => self.change_preset(-1, false),
                Key::Character(c) => match c.as_str() {
                    "n" => self.change_preset(1, false),
                    "p" => self.change_preset(-1, false),
                    "r" => self.change_preset(0, true),
                    "q" => event_loop.exit(),
                    _ => {}
                },
                _ => {}
            },
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(render) = &self.render {
            render.window.request_redraw();
        }
    }
}

fn collect_presets(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_presets(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("milk")) {
            out.push(path);
        }
    }
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_DIR.to_string());
    let mut presets = Vec::new();
    collect_presets(Path::new(&dir), &mut presets);
    if presets.is_empty() {
        eprintln!("No .milk presets found in {dir:?}; using the built-in preset.");
    } else {
        println!("Loaded {} presets from {dir}", presets.len());
    }
    println!("Keys: Right/Space/N next · Left/P prev · R random · Esc/Q quit");

    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(presets);
    event_loop.run_app(&mut app).expect("run app");
}
