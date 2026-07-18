//! `pm-web` — the browser/WASM frontend adapter for projectM-rs.
//!
//! This crate owns only the browser-specific seams (canvas + async WebGPU init,
//! render loop, the Web Audio bridge, and — from later phases — storage and URL
//! import/export). The visualizer engine itself lives in the platform-neutral
//! `pm-*` crates and is reused here directly.
//!
//! The whole crate is `cfg`-gated to `wasm32`, so a native `cargo build
//! --workspace` compiles it to an empty library and `pm-app` is unaffected.
//!
//! Audio bridge (Phase 3): an `AudioWorklet` on the browser audio thread writes
//! normalized PCM into a lock-free SPSC ring in a `SharedArrayBuffer`. This
//! wasm side drains the ring each rendered frame and feeds the existing
//! `pm_audio::PCM` seam — the projectM FFT/beat/waveform analysis is unchanged;
//! only the sample *source* differs from the native `cpal` path.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

use pm_audio::PCM;
use pm_core::{PresetPlayer, WarpEngine, BUILTIN_PRESET, DEFAULT_TRANSITION_SECS};
use pm_preset::Preset;
use pm_render::{Blit, GpuContext};

// ---------------------------------------------------------------------------
// Audio bridge: lock-free SPSC ring drained from the render loop.
//
// The ring lives in two SharedArrayBuffers owned by JS. `control` is a 6-slot
// Int32Array addressed with Atomics; `data` is a Float32Array of interleaved
// samples. The AudioWorklet is the sole producer; this wasm side is the sole
// consumer. Indices are in *samples* (not frames).
// ---------------------------------------------------------------------------

// control slot indices
const C_WRITE: u32 = 0;
const C_READ: u32 = 1;
const C_OVERRUNS: u32 = 2;
const C_UNDERRUNS: u32 = 3;
const C_CHANNELS: u32 = 4;
const C_SAMPLE_RATE: u32 = 5;

struct AudioBridge {
    control: js_sys::Int32Array,
    data: js_sys::Float32Array,
    capacity: i32,
    consumed: u64,
}

impl AudioBridge {
    fn load(&self, idx: u32) -> i32 {
        js_sys::Atomics::load(&self.control, idx).unwrap_or(0)
    }

    /// Drain all currently-available whole frames into `out`. Returns
    /// `(channels, sample_count)`. A partial trailing frame (if any) is left in
    /// the ring for the next drain. Bumps the underrun counter when starved.
    fn drain_into(&mut self, out: &mut Vec<f32>) -> (u32, usize) {
        let write = self.load(C_WRITE);
        let read = self.load(C_READ);
        let channels = self.load(C_CHANNELS).max(1) as u32;
        let cap = self.capacity;

        let mut available = write - read;
        if available < 0 {
            available += cap;
        }
        // Align down to whole interleaved frames so a stereo pair never splits.
        let ch = channels as i32;
        let n = ((available / ch) * ch) as usize;
        if n == 0 {
            let _ = js_sys::Atomics::add(&self.control, C_UNDERRUNS, 1);
            return (channels, 0);
        }
        if out.len() < n {
            out.resize(n, 0.0);
        }

        // Copy [read, read+n) with wrap-around, in at most two bulk copies.
        let first = std::cmp::min(n as i32, cap - read);
        self.data
            .subarray(read as u32, (read + first) as u32)
            .copy_to(&mut out[..first as usize]);
        if (first as usize) < n {
            let rem = n - first as usize;
            self.data
                .subarray(0, rem as u32)
                .copy_to(&mut out[first as usize..n]);
        }

        let new_read = (read + n as i32) % cap;
        let _ = js_sys::Atomics::store(&self.control, C_READ, new_read);
        self.consumed += n as u64;
        (channels, n)
    }

    fn fill(&self) -> f32 {
        let mut a = self.load(C_WRITE) - self.load(C_READ);
        if a < 0 {
            a += self.capacity;
        }
        a as f32 / self.capacity.max(1) as f32
    }
}

/// Rust-side diagnostics snapshot, updated each frame and read by the JS panel.
#[derive(Default, Clone)]
struct Diagnostics {
    has_audio: bool,
    channels: u32,
    sample_rate: i32,
    ring_fill: f32,
    overruns: i32,
    underruns: i32,
    consumed: u64,
    bass: f32,
    mid: f32,
    treb: f32,
    vol: f32,
}

thread_local! {
    static AUDIO: RefCell<Option<AudioBridge>> = const { RefCell::new(None) };
    static DIAG: RefCell<Diagnostics> = RefCell::new(Diagnostics::default());
}

/// Attach a ring buffer produced by the JS audio graph. `control` is the 6-slot
/// Int32Array; `data` the interleaved Float32 ring; `capacity` its length in
/// samples. Called from a user gesture once an `AudioContext` + worklet exist.
#[wasm_bindgen]
pub fn set_audio_ring(control: js_sys::Int32Array, data: js_sys::Float32Array, capacity: u32) {
    let bridge = AudioBridge {
        control,
        data,
        capacity: capacity as i32,
        consumed: 0,
    };
    AUDIO.with(|a| *a.borrow_mut() = Some(bridge));
    log::info!("pm-web: audio ring attached ({capacity} samples)");
}

/// Detach the ring (source disabled). The render loop continues; audio values
/// decay to silence. Never affects the renderer.
#[wasm_bindgen]
pub fn clear_audio() {
    AUDIO.with(|a| *a.borrow_mut() = None);
    log::info!("pm-web: audio ring detached");
}

/// Diagnostics for the JS panel, as a JSON string (avoids a serde dependency).
/// The JS side merges its own AudioContext state / `crossOriginIsolated`.
#[wasm_bindgen]
pub fn get_diagnostics() -> String {
    DIAG.with(|d| {
        let d = d.borrow();
        format!(
            "{{\"hasAudio\":{},\"channels\":{},\"sampleRate\":{},\"ringFill\":{:.3},\
             \"overruns\":{},\"underruns\":{},\"consumed\":{},\
             \"bass\":{:.4},\"mid\":{:.4},\"treb\":{:.4},\"vol\":{:.4}}}",
            d.has_audio,
            d.channels,
            d.sample_rate,
            d.ring_fill,
            d.overruns,
            d.underruns,
            d.consumed,
            d.bass,
            d.mid,
            d.treb,
            d.vol,
        )
    })
}

// ---------------------------------------------------------------------------
// Boot + render loop
// ---------------------------------------------------------------------------

/// Module entry point: install panic hook + console logger. Runs once when the
/// wasm module is instantiated.
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    log::info!("pm-web wasm module loaded");
}

/// Whether the browser exposes the WebGPU API (`navigator.gpu`).
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

    // Field initializers run in order: `blit`/`player` borrow `&ctx` (released
    // per-expression), then `ctx` is moved into the struct.
    let state = Rc::new(RefCell::new(State {
        blit: Blit::new(&ctx, format),
        player: build_player(&ctx, width, height),
        ctx,
        surface,
        config,
        canvas,
        pcm: PCM::new(),
        audio_scratch: Vec::new(),
        time: 0.0,
        frame: 0,
    }));

    // Self-sustaining requestAnimationFrame loop.
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

/// Build a fresh player for the built-in preset at the given size.
fn build_player(ctx: &GpuContext, width: u32, height: u32) -> PresetPlayer {
    let preset = Preset::load(BUILTIN_PRESET).expect("built-in preset parses");
    let engine = WarpEngine::new(ctx, preset, width, height);
    PresetPlayer::new(ctx, engine, width, height, DEFAULT_TRANSITION_SECS)
}

/// Everything the render loop needs to draw, analyze audio, and recover.
struct State {
    ctx: GpuContext,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    player: PresetPlayer,
    blit: Blit,
    canvas: HtmlCanvasElement,
    pcm: PCM,
    audio_scratch: Vec<f32>,
    time: f32,
    frame: u32,
}

impl State {
    fn render(&mut self) {
        // Resize: reconfigure the surface + rebuild the size-bound engine.
        let w = self.canvas.width().max(1);
        let h = self.canvas.height().max(1);
        if w != self.config.width || h != self.config.height {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.ctx.device, &self.config);
            self.player = build_player(&self.ctx, w, h);
            self.time = 0.0;
        }

        // Drain the audio ring (if attached) into the projectM PCM analyzer.
        let mut has_audio = false;
        let mut channels = 0u32;
        let mut count = 0usize;
        AUDIO.with(|a| {
            if let Some(bridge) = a.borrow_mut().as_mut() {
                has_audio = true;
                let (c, n) = bridge.drain_into(&mut self.audio_scratch);
                channels = c;
                count = n;
            }
        });
        if count > 0 {
            self.pcm.add_float(&self.audio_scratch[..count], channels);
        }

        // Fixed-timestep engine clock + one analysis step per frame.
        self.time += 1.0 / 60.0;
        self.pcm.update_frame_audio_data(1.0 / 60.0, self.frame);
        self.frame = self.frame.wrapping_add(1);
        let audio = self.pcm.frame_audio_data();

        // Publish diagnostics for the JS panel.
        let (fill, overruns, underruns, sample_rate, consumed) = AUDIO.with(|a| {
            match a.borrow().as_ref() {
                Some(b) => (b.fill(), b.load(C_OVERRUNS), b.load(C_UNDERRUNS), b.load(C_SAMPLE_RATE), b.consumed),
                None => (0.0, 0, 0, 0, 0),
            }
        });
        DIAG.with(|d| {
            let mut d = d.borrow_mut();
            d.has_audio = has_audio;
            d.channels = channels;
            d.sample_rate = sample_rate;
            d.ring_fill = fill;
            d.overruns = overruns;
            d.underruns = underruns;
            d.consumed = consumed;
            d.bass = audio.bass;
            d.mid = audio.mid;
            d.treb = audio.treb;
            d.vol = audio.vol;
        });

        self.player.render(&self.ctx, self.time, audio);

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.ctx.device, &self.config);
                return;
            }
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
