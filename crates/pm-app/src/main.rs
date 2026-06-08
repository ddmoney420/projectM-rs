//! `pm-app` — a live window that renders Milkdrop presets reacting to audio.
//!
//! ```text
//! cargo run -p pm-app --release -- [preset-dir]
//! ```
//!
//! Keys: →/Space/N next preset · ←/P previous · R random · T toggle
//! transitions · Esc/Q quit.

mod audio;
mod blit;

use audio::AudioInput;
use blit::Blit;
use pm_core::{PresetPlayer, WarpEngine, DEFAULT_TRANSITION_SECS};
use pm_preset::Preset;
use pm_render::wgpu;
use pm_render::{read_rgba8, GpuContext};
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
    player: PresetPlayer,
    blit: Blit,
}

/// Number of warm-up frames to build feedback content before probing.
const WARMUP_FRAMES: i32 = 8;
/// Max presets to skip when searching for one that renders visible content.
const MAX_PROBE: usize = 400;
/// A preset "renders" if at least this many display pixels are non-black.
const CONTENT_THRESHOLD: usize = 400;

struct App {
    presets: Vec<PathBuf>,
    index: usize,
    /// False until the user navigates into the corpus (we start on the built-in).
    on_corpus: bool,
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
            on_corpus: false,
            rng: 0x1234_5678,
            audio: AudioInput::new(),
            render: None,
            start: now,
            last: now,
            frame: 0,
        }
    }

    /// The current preset: the built-in until the user navigates into the corpus.
    fn current_preset(&self) -> (Preset, String) {
        if self.on_corpus {
            if let Some(path) = self.presets.get(self.index) {
                if let Ok(bytes) = std::fs::read(path) {
                    if let Ok(preset) = Preset::load(&String::from_utf8_lossy(&bytes)) {
                        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        return (preset, name);
                    }
                }
            }
        }
        (Preset::load(BUILTIN).expect("builtin preset"), "built-in".into())
    }

    /// Navigate the corpus, skipping presets that render black (many advanced
    /// presets need content generators we don't render yet).
    fn change_preset(&mut self, delta: i64, random: bool) {
        if self.presets.is_empty() {
            return;
        }
        let len = self.presets.len();
        let dir: i64 = if delta < 0 { -1 } else { 1 };
        let start = if random {
            self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (self.rng as usize) % len
        } else if !self.on_corpus {
            // First step into the corpus: begin at an end.
            if dir > 0 { 0 } else { len - 1 }
        } else {
            ((self.index as i64 + delta).rem_euclid(len as i64)) as usize
        };
        self.on_corpus = true;

        let Some(render) = self.render.as_mut() else { return };
        let (w, h) = (render.config.width, render.config.height);
        let (engine, idx, name) =
            find_renderable(&render.ctx, w, h, &self.presets, &mut self.audio, self.frame, start, dir);
        let cc = if engine.uses_custom_composite() { " ·custom-comp" } else { "" };
        render.player.switch_to(engine);
        render.window.set_title(&format!("pm-app — {name}{cc}  [{}/{}]", idx + 1, len));
        println!("Showing [{}/{}]: {name}", idx + 1, len);
        self.index = idx;
    }

    /// Toggle smooth preset transitions on/off (off = instant hard cut).
    fn toggle_transitions(&mut self) {
        if let Some(render) = &mut self.render {
            let on = render.player.duration() > 0.0;
            render.player.set_duration(if on { 0.0 } else { DEFAULT_TRANSITION_SECS });
            println!("Preset transitions: {}", if on { "off (hard cut)" } else { "on (2.7s)" });
        }
    }

    /// Recreate the warp engine for the current preset at the current size.
    fn rebuild_engine(&mut self) {
        let (preset, name) = self.current_preset();
        if let Some(render) = &mut self.render {
            let (w, h) = (render.config.width, render.config.height);
            let engine = WarpEngine::new(&render.ctx, preset, w, h);
            let cc = if engine.uses_custom_composite() { " ·custom-comp" } else { "" };
            // Resize is a clean reset, not a transition.
            let duration = render.player.duration();
            render.player = PresetPlayer::new(&render.ctx, engine, w, h, duration);
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
        render.player.render(&render.ctx, time, audio);
        self.frame += 1;

        match render.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                render.blit.draw(&render.ctx, render.player.output_texture(), &view);
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
        let player = PresetPlayer::new(&ctx, engine, w, h, DEFAULT_TRANSITION_SECS);

        self.render = Some(Render { window, ctx, surface, config, player, blit });
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
                    "t" => self.toggle_transitions(),
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

/// Load a preset file, returning the parsed preset and its display name.
fn load_preset(path: &Path) -> Option<(Preset, String)> {
    let bytes = std::fs::read(path).ok()?;
    let preset = Preset::load(&String::from_utf8_lossy(&bytes)).ok()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some((preset, name))
}

/// Run a few frames so the feedback buffer accumulates content.
fn warm_up(engine: &mut WarpEngine, ctx: &GpuContext, audio: &mut AudioInput, base: i32) {
    for f in 0..WARMUP_FRAMES {
        let n = base + f;
        let a = audio.frame_data(1.0 / 60.0, n);
        let _ = engine.render_frame(ctx, n as f32 / 60.0, n, a);
    }
}

/// True if the engine's display output has visible (non-black) content.
fn has_content(ctx: &GpuContext, engine: &WarpEngine) -> bool {
    let px = read_rgba8(ctx, engine.display_texture());
    px.chunks_exact(4)
        .filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30)
        .count()
        > CONTENT_THRESHOLD
}

/// Resolution used for the cheap "does it render anything?" probe.
const PROBE_SIZE: u32 = 160;

/// Search from `start` in direction `dir` for a preset that renders visible
/// content. Each candidate is probed at low resolution (fast); the winner is
/// rebuilt at full resolution. Falls back to the built-in if none found.
#[allow(clippy::too_many_arguments)]
fn find_renderable(
    ctx: &GpuContext,
    w: u32,
    h: u32,
    presets: &[PathBuf],
    audio: &mut AudioInput,
    frame_base: i32,
    start: usize,
    dir: i64,
) -> (WarpEngine, usize, String) {
    let len = presets.len();
    for attempt in 0..MAX_PROBE.min(len) {
        let idx = ((start as i64 + dir * attempt as i64).rem_euclid(len as i64)) as usize;
        let Some((preset, name)) = load_preset(&presets[idx]) else { continue };

        let mut probe = WarpEngine::new(ctx, preset, PROBE_SIZE, PROBE_SIZE);
        warm_up(&mut probe, ctx, audio, frame_base);
        if has_content(ctx, &probe) {
            // Rebuild the winner at full resolution.
            let (preset, _) = load_preset(&presets[idx]).expect("reload");
            let mut engine = WarpEngine::new(ctx, preset, w, h);
            warm_up(&mut engine, ctx, audio, frame_base);
            return (engine, idx, name);
        }
    }
    // Nothing rendered in range — show the built-in instead.
    let mut engine = WarpEngine::new(ctx, Preset::load(BUILTIN).unwrap(), w, h);
    warm_up(&mut engine, ctx, audio, frame_base);
    (engine, start.min(len.saturating_sub(1)), "built-in".into())
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
    println!("Keys: Right/Space/N next · Left/P prev · R random · T transitions · Esc/Q quit");

    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(presets);
    event_loop.run_app(&mut app).expect("run app");
}
