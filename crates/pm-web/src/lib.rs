//! `pm-web` — WebAssembly entry point for the projectM-rs engine.
//!
//! This is the browser counterpart to `pm-app`: it drives the exact same
//! engine (`WarpEngine::render_frame`) but swaps the four platform edges —
//!
//! | native (`pm-app`)        | web (`pm-web`)                       |
//! |--------------------------|--------------------------------------|
//! | winit window + surface   | `HtmlCanvasElement` surface          |
//! | `pollster::block_on`     | `async` / `wasm-bindgen-futures`     |
//! | cpal mic input           | silence today (Web Audio = next step)|
//! | `requestAnimationFrame`  | `requestAnimationFrame`              |
//!
//! Increment 1: boot wgpu on a canvas, load a caller-supplied `.milk` preset,
//! and render it to the canvas every frame with silent audio. Audio capture
//! (Web Audio `AnalyserNode`) and preset streaming (fetch/IndexedDB) land next.

use std::cell::RefCell;
use std::rc::Rc;

use pm_audio::FrameAudioData;
use pm_core::WarpEngine;
use pm_preset::Preset;
use pm_render::wgpu;
use pm_render::{GpuContext, Texture};
use wasm_bindgen::prelude::*;

mod blit;
use blit::Blit;

/// One-time browser setup: route panics and `log` to the devtools console.
#[wasm_bindgen(start)]
pub fn start() {
    #[cfg(target_arch = "wasm32")]
    {
        console_error_panic_hook::set_once();
        let _ = console_log::init_with_level(log::Level::Info);
    }
}

/// Boot the engine on `canvas`, rendering `preset_text` (a `.milk` preset).
///
/// Returns once the render loop is installed; the loop then runs forever via
/// `requestAnimationFrame`. Errors (bad preset, no GPU adapter) surface as a
/// rejected JS promise.
#[wasm_bindgen]
pub async fn run(canvas: web_sys::HtmlCanvasElement, preset_text: String) -> Result<(), JsValue> {
    let width = canvas.width().max(1);
    let height = canvas.height().max(1);

    // --- wgpu init: the pm-app sequence, with create_surface from a canvas
    //     and `.await` instead of pollster::block_on. ---
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|e| JsValue::from_str(&format!("create_surface: {e}")))?;

    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        })
        .await
        .map_err(|e| JsValue::from_str(&format!("request_adapter: {e}")))?;

    // WebGL2 caps are lower than native; ask for limits the surface supports so
    // the device request doesn't over-reach on mobile browsers.
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("pm-web device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                .using_resolution(adapter.limits()),
            ..Default::default()
        })
        .await
        .map_err(|e| JsValue::from_str(&format!("request_device: {e}")))?;

    let caps = surface.get_capabilities(&adapter);
    let surface_format = caps.formats[0];
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width,
        height,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
    };
    surface.configure(&device, &config);

    let ctx = GpuContext { instance, adapter, device, queue };

    // --- engine init: identical to native ---
    let preset =
        Preset::load(&preset_text).map_err(|e| JsValue::from_str(&format!("preset: {e:?}")))?;
    let engine = WarpEngine::new(&ctx, preset, width, height);
    let blit = Blit::new(&ctx, surface_format);

    log::info!("pm-web: engine up ({width}x{height}) on {surface_format:?}");

    // --- render loop via requestAnimationFrame ---
    // The classic Rc<RefCell<Closure>> self-rescheduling pattern: the closure
    // holds a handle to itself so it can request the next frame.
    let state = Rc::new(RefCell::new(FrameState {
        ctx,
        surface,
        engine,
        blit,
        frame: 0,
    }));

    let cb: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let cb_clone = cb.clone();
    *cb.borrow_mut() = Some(Closure::wrap(Box::new(move || {
        {
            let mut s = state.borrow_mut();
            s.render();
        }
        request_animation_frame(cb_clone.borrow().as_ref().unwrap());
    }) as Box<dyn FnMut()>));
    request_animation_frame(cb.borrow().as_ref().unwrap());

    Ok(())
}

struct FrameState {
    ctx: GpuContext,
    surface: wgpu::Surface<'static>,
    engine: WarpEngine,
    blit: Blit,
    frame: i32,
}

impl FrameState {
    fn render(&mut self) {
        let time = self.frame as f32 / 60.0;
        // Increment 1: silent audio. Web Audio AnalyserNode feeds this next.
        if let Err(e) = self.engine.render_frame(&self.ctx, time, self.frame, FrameAudioData::default()) {
            log::error!("render_frame: {e:?}");
            return;
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // Surface needs reconfiguring (e.g. canvas resized/backgrounded);
                // skip this frame and try again next rAF tick.
                log::warn!("surface outdated/lost — skipping frame");
                return;
            }
            // Timeout / Occluded / Validation: nothing usable this tick, retry next rAF.
            other => {
                log::warn!("surface unavailable this frame: {other:?}");
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.blit.draw(&self.ctx, self.engine.display_texture(), &view);
        frame.present();
        self.frame += 1;
    }
}

fn request_animation_frame(cb: &Closure<dyn FnMut()>) {
    web_sys::window()
        .expect("no window")
        .request_animation_frame(cb.as_ref().unchecked_ref())
        .expect("requestAnimationFrame failed");
}

// Re-export so downstream (the React app) can sanity-check the engine output
// type without reaching into pm-render directly.
pub use pm_render::Texture as EngineTexture;
#[allow(unused_imports)]
use Texture as _; // keep the import meaningful if blit's signature changes
