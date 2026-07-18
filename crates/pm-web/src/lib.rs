//! `pm-web` — the browser/WASM frontend adapter for projectM-rs.
//!
//! This crate owns only the browser-specific seams (canvas + async WebGPU init,
//! render loop, and — from later phases — the Web Audio bridge, storage, and URL
//! import/export). The visualizer engine itself lives in the platform-neutral
//! `pm-*` crates and is reused here directly.
//!
//! The whole crate is `cfg`-gated to `wasm32`, so a native `cargo build
//! --workspace` compiles it to an empty library and `pm-app` is unaffected.
//!
//! Phase 2 scope (this file): construct a [`GpuContext`] from a `<canvas>`
//! surface (async, no `pollster`), build the engine's built-in preset, and drive
//! [`PresetPlayer`] each frame — blitting its output to the canvas with the
//! shared [`Blit`]. Audio is `FrameAudioData::default()` for now (silence); the
//! Web Audio bridge lands in Phase 3.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

use pm_audio::FrameAudioData;
use pm_core::{PresetPlayer, WarpEngine, BUILTIN_PRESET, DEFAULT_TRANSITION_SECS};
use pm_preset::Preset;
use pm_render::{Blit, GpuContext};

/// Module entry point: install panic hook + console logger. Runs once when the
/// wasm module is instantiated.
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    log::info!("pm-web wasm module loaded");
}

/// Whether the browser exposes the WebGPU API (`navigator.gpu`). The JS shell
/// calls this to decide between booting the app and showing the
/// unsupported-WebGPU page.
#[wasm_bindgen]
pub fn webgpu_supported() -> bool {
    let Some(win) = web_sys::window() else { return false };
    let nav: JsValue = win.navigator().into();
    match js_sys::Reflect::get(&nav, &JsValue::from_str("gpu")) {
        Ok(v) => !v.is_undefined() && !v.is_null(),
        Err(_) => false,
    }
}

/// Boot the visualizer on the canvas with the given DOM id. Async because
/// WebGPU adapter/device acquisition is Promise-based; `pollster` cannot block
/// the browser main thread. Returns an error (surfaced to JS) if WebGPU is
/// unavailable or initialization fails, so the shell can show the fallback page.
#[wasm_bindgen]
pub async fn run(canvas_id: String) -> Result<(), JsValue> {
    let win = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let doc = win.document().ok_or_else(|| JsValue::from_str("no document"))?;
    let canvas: HtmlCanvasElement = doc
        .get_element_by_id(&canvas_id)
        .ok_or_else(|| JsValue::from_str(&format!("canvas #{canvas_id} not found")))?
        .dyn_into()
        .map_err(|_| JsValue::from_str("element is not a <canvas>"))?;

    // On wasm the default instance selects the WebGPU backend.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());

    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
        .map_err(|e| JsValue::from_str(&format!("create_surface failed: {e}")))?;

    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        })
        .await
        .map_err(|e| JsValue::from_str(&format!("no WebGPU adapter: {e}")))?;

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("pm-web device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        })
        .await
        .map_err(|e| JsValue::from_str(&format!("request_device failed: {e}")))?;

    let caps = surface.get_capabilities(&adapter);
    let format = caps.formats[0];
    let width = canvas.width().max(1);
    let height = canvas.height().max(1);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width,
        height,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
    };

    let ctx = GpuContext { instance, adapter, device, queue };
    surface.configure(&ctx.device, &config);

    log::info!("pm-web: engine initialized ({width}x{height}); starting render loop");

    // Field initializers run in order: `blit` and `player` borrow `&ctx`
    // (borrows released per-expression), then `ctx` is moved into the struct.
    let state = Rc::new(RefCell::new(State {
        blit: Blit::new(&ctx, format),
        player: build_player(&ctx, width, height),
        ctx,
        surface,
        config,
        canvas,
        time: 0.0,
    }));

    // Self-sustaining requestAnimationFrame loop: the closure holds an `Rc` to
    // itself (via `f`), which intentionally keeps it alive after `run` returns.
    let f: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let g = f.clone();
    let st = state.clone();
    *g.borrow_mut() = Some(Closure::wrap(Box::new(move || {
        st.borrow_mut().render();
        request_animation_frame(f.borrow().as_ref().unwrap());
    }) as Box<dyn FnMut()>));
    request_animation_frame(g.borrow().as_ref().unwrap());

    Ok(())
}

/// Build a fresh player for the built-in preset at the given size. Called at
/// startup and whenever the canvas resizes (the warp/feedback buffers are
/// size-bound, so a resize rebuilds the engine).
fn build_player(ctx: &GpuContext, width: u32, height: u32) -> PresetPlayer {
    let preset = Preset::load(BUILTIN_PRESET).expect("built-in preset parses");
    let engine = WarpEngine::new(ctx, preset, width, height);
    PresetPlayer::new(ctx, engine, width, height, DEFAULT_TRANSITION_SECS)
}

/// Everything the render loop needs to draw and recover across frames.
struct State {
    ctx: GpuContext,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    player: PresetPlayer,
    blit: Blit,
    canvas: HtmlCanvasElement,
    time: f32,
}

impl State {
    fn render(&mut self) {
        // Track the canvas backing-store size (the JS shell applies DPR), and
        // reconfigure + rebuild on any change so resizes are handled here.
        let w = self.canvas.width().max(1);
        let h = self.canvas.height().max(1);
        if w != self.config.width || h != self.config.height {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.ctx.device, &self.config);
            self.player = build_player(&self.ctx, w, h);
            self.time = 0.0;
        }

        // Fixed-timestep engine clock (deterministic, browser-clock-independent).
        self.time += 1.0 / 60.0;
        self.player
            .render(&self.ctx, self.time, FrameAudioData::default());

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            // Lost/Outdated: reconfigure and skip this frame; the next tick draws.
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.ctx.device, &self.config);
                return;
            }
            // Timeout / Occluded / Validation: skip this frame.
            _ => return,
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.blit
            .draw(&self.ctx, self.player.output_texture(), &view);
        frame.present();
    }
}

fn request_animation_frame(f: &Closure<dyn FnMut()>) {
    web_sys::window()
        .expect("no window")
        .request_animation_frame(f.as_ref().unchecked_ref())
        .expect("request_animation_frame failed");
}
