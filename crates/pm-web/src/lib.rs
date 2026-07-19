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

mod compositor;
mod live_shader;
mod overlay;

use compositor::{Compositor, LayerKind};
use pm_audio::PCM;
use pm_core::{PresetPlayer, WarpEngine, BUILTIN_PRESET, DEFAULT_TRANSITION_SECS};
use pm_glsl::{ShaderMode, ShaderUniforms, AUDIO_TEX_HEIGHT, AUDIO_TEX_WIDTH};
use pm_params::{AudioFeatures, Curve, Lfo, LfoWave, ModContext, ModSource, Parameter, Tempo, VisualClock};
use pm_preset::Preset;
use pm_render::GpuContext;
use pm_scene::OverlayConfig;

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
    time: f32,
    delta: f32,
    scale: f32,
    paused: bool,
    frame: u32,
    width: u32,
    height: u32,
    layer_count: u32,
    enabled_count: u32,
    shader_count: u32,
    bpm: f32,
    beat_phase: f32,
    beat_pulse: f32,
    tempo_confidence: f32,
    tempo_manual: bool,
}

thread_local! {
    static AUDIO: RefCell<Option<AudioBridge>> = const { RefCell::new(None) };
    static DIAG: RefCell<Diagnostics> = RefCell::new(Diagnostics::default());
    /// The running app, so `#[wasm_bindgen]` exports (shader console, render
    /// source, mouse, time controls) can reach the render state.
    static APP: RefCell<Option<Rc<RefCell<State>>>> = const { RefCell::new(None) };
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
             \"bass\":{:.4},\"mid\":{:.4},\"treb\":{:.4},\"vol\":{:.4},\
             \"time\":{:.2},\"delta\":{:.4},\"scale\":{:.2},\"paused\":{},\
             \"frame\":{},\"width\":{},\"height\":{},\
             \"layerCount\":{},\"enabledCount\":{},\"shaderCount\":{},\
             \"bpm\":{:.1},\"beatPhase\":{:.3},\"beatPulse\":{:.3},\
             \"tempoConfidence\":{:.2},\"tempoManual\":{}}}",
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
            d.time,
            d.delta,
            d.scale,
            d.paused,
            d.frame,
            d.width,
            d.height,
            d.layer_count,
            d.enabled_count,
            d.shader_count,
            d.bpm,
            d.beat_phase,
            d.beat_pulse,
            d.tempo_confidence,
            d.tempo_manual,
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

    let state = Rc::new(RefCell::new(State {
        player: build_player(&ctx, width, height),
        compositor: Compositor::new(&ctx, format, width, height),
        ctx,
        surface,
        config,
        canvas,
        pcm: PCM::new(),
        audio_scratch: Vec::new(),
        mouse: [0.0; 4],
        clock: VisualClock::new(),
        tempo: Tempo::default(),
        lfos: [Lfo::default(), Lfo::default(), Lfo::default(), Lfo::default()],
        frame: 0,
    }));
    APP.with(|a| *a.borrow_mut() = Some(state.clone()));

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
    /// The single shared Milkdrop engine (one Milkdrop layer, by constraint).
    player: PresetPlayer,
    compositor: Compositor,
    canvas: HtmlCanvasElement,
    pcm: PCM,
    audio_scratch: Vec<f32>,
    /// iMouse: (x, y, click_x, click_y) in canvas pixels, y bottom-left origin.
    mouse: [f32; 4],
    clock: VisualClock,
    tempo: Tempo,
    lfos: [Lfo; 4],
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
            self.compositor.resize(&self.ctx, w, h);
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

        // Audio analysis advances at real time (fixed 1/60 step — immune to
        // tab-suspension delta spikes).
        self.pcm.update_frame_audio_data(1.0 / 60.0, self.frame);
        let audio = self.pcm.frame_audio_data();
        self.frame = self.frame.wrapping_add(1);

        // Controlled visual clock (pause/scale), tempo, and LFO bank.
        let visual_dt = self.clock.tick(1.0 / 60.0);
        let feat = AudioFeatures {
            bass: audio.bass,
            mid: audio.mid,
            treb: audio.treb,
            vol: audio.vol,
            bass_att: audio.bass_att,
            mid_att: audio.mid_att,
            treb_att: audio.treb_att,
            vol_att: audio.vol_att,
        };
        self.tempo.update(visual_dt, &feat);
        let beat_time = self.tempo.beat_time();
        let mut lfo_vals = [0.0f32; 4];
        for (i, lfo) in self.lfos.iter_mut().enumerate() {
            lfo_vals[i] = lfo.update(visual_dt, beat_time);
        }
        let modctx = ModContext {
            audio: feat,
            beat_phase: self.tempo.beat_phase(),
            beat_pulse: self.tempo.beat_pulse(),
            lfo: lfo_vals,
        };

        let time = self.clock.time();

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
            d.time = time;
            d.delta = visual_dt;
            d.scale = self.clock.scale();
            d.paused = self.clock.paused();
            d.frame = self.frame;
            d.width = self.config.width;
            d.height = self.config.height;
            d.layer_count = self.compositor.layer_count() as u32;
            d.enabled_count = self.compositor.enabled_count() as u32;
            d.shader_count = self.compositor.shader_count() as u32;
            d.bpm = self.tempo.bpm();
            d.beat_phase = self.tempo.beat_phase();
            d.beat_pulse = self.tempo.beat_pulse();
            d.tempo_confidence = self.tempo.confidence();
            d.tempo_manual = self.tempo.manual();
        });

        // Shadertoy uniform snapshot for this frame.
        let uniforms = ShaderUniforms {
            i_resolution: [self.config.width as f32, self.config.height as f32, 1.0],
            i_time: time,
            i_mouse: self.mouse,
            i_date: date_vec4(),
            i_time_delta: visual_dt,
            i_frame: self.frame as f32,
            i_sample_rate: sample_rate as f32,
            pm_pad0: 0.0,
            i_channel_resolution: [[AUDIO_TEX_WIDTH as f32, AUDIO_TEX_HEIGHT as f32, 1.0, 0.0]; 4],
            i_bass: audio.bass,
            i_mid: audio.mid,
            i_treb: audio.treb,
            i_vol: audio.vol,
            i_bass_att: audio.bass_att,
            i_mid_att: audio.mid_att,
            i_treb_att: audio.treb_att,
            i_vol_att: audio.vol_att,
            i_bpm: self.tempo.bpm(),
            i_beat_phase: self.tempo.beat_phase(),
            i_beat_pulse: self.tempo.beat_pulse(),
            i_beat_index: (self.tempo.beat_index() % 1_000_000) as f32,
            i_bar_phase: self.tempo.bar_phase(),
            i_tempo_confidence: self.tempo.confidence(),
            pm_pad1: 0.0,
            pm_pad2: 0.0,
        };

        // Render the shared Milkdrop engine once if a Milkdrop layer exists.
        if self.compositor.has_milkdrop() {
            self.player.render(&self.ctx, time, audio.clone());
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.ctx.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let player_out = self.player.output_texture();
        self.compositor
            .render(&self.ctx, &view, player_out, &audio, &uniforms, &modctx);
        frame.present();
    }
}

/// iDate as Shadertoy expects: (year, month0-11, day-of-month, seconds-since-midnight).
fn date_vec4() -> [f32; 4] {
    let d = js_sys::Date::new_0();
    let secs = d.get_hours() as f64 * 3600.0
        + d.get_minutes() as f64 * 60.0
        + d.get_seconds() as f64
        + d.get_milliseconds() as f64 / 1000.0;
    [
        d.get_full_year() as f32,
        d.get_month() as f32,
        d.get_date() as f32,
        secs as f32,
    ]
}

/// Run a closure with the live app state, if it's been initialized.
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    APP.with(|a| a.borrow().as_ref().map(|s| f(&mut s.borrow_mut())))
}

/// Compile GLSL into the **selected** shader layer (mode 0 = Shadertoy, 1 = raw).
/// Synchronous: a newer call wins; a failed compile keeps that layer's
/// last-known-good and never affects other layers, audio, or the render loop.
#[wasm_bindgen]
pub fn set_shader_source(mode: u8, src: String) -> String {
    let m = if mode == 1 { ShaderMode::Raw } else { ShaderMode::Shadertoy };
    let t0 = js_sys::Date::now();
    let outcome = with_state(|s| {
        let id = s.compositor.selected()?;
        s.compositor.set_shader(&s.ctx, id, m, &src)
    })
    .flatten();
    let ms = js_sys::Date::now() - t0;
    match outcome {
        Some(o) => {
            let diags: Vec<String> = o
                .diagnostics
                .iter()
                .map(|d| format!("{{\"line\":{},\"column\":{},\"message\":{}}}", d.line, d.column, json_string(&d.message)))
                .collect();
            let controls: Vec<String> = o.controls.iter().map(control_json).collect();
            format!(
                "{{\"ok\":{},\"compileMs\":{:.1},\"diagnostics\":[{}],\"controls\":[{}]}}",
                o.ok,
                ms,
                diags.join(","),
                controls.join(",")
            )
        }
        None => "{\"ok\":false,\"compileMs\":0,\"diagnostics\":[{\"line\":0,\"column\":0,\"message\":\"select a shader layer first\"}],\"controls\":[]}".to_string(),
    }
}

fn control_json(c: &pm_glsl::Control) -> String {
    let opts: Vec<String> = c.options.iter().map(|o| json_string(o)).collect();
    format!(
        "{{\"name\":{},\"kind\":\"{}\",\"min\":{},\"max\":{},\"slot\":{},\
         \"default\":[{},{},{},{}],\"options\":[{}]}}",
        json_string(&c.name),
        c.kind.as_str(),
        c.min,
        c.max,
        c.slot,
        c.default[0],
        c.default[1],
        c.default[2],
        c.default[3],
        opts.join(",")
    )
}

/// Set a user control's base value (all four `vec4` lanes) on the selected layer.
#[wasm_bindgen]
pub fn set_control(index: u32, x: f32, y: f32, z: f32, w: f32) {
    with_state(|s| {
        if let Some(id) = s.compositor.selected() {
            s.compositor.set_control(id, index as usize, [x, y, z, w]);
        }
    });
}

/// Bind (or clear) modulation on a scalar user control of the selected layer.
/// `source` is a [`ModSource`] name; `curve` one of linear/exp/log/scurve.
#[wasm_bindgen]
pub fn set_control_mod(index: u32, source: String, amount: f32, smoothing: f32, curve: String, invert: bool) {
    with_state(|s| {
        let Some(id) = s.compositor.selected() else { return };
        let i = index as usize;
        let src = ModSource::from_str(&source);
        if matches!(src, ModSource::None) && amount == 0.0 {
            s.compositor.set_control_mod(id, i, None);
            return;
        }
        let [min, max] = s.compositor.user_range(id, i);
        let mut p = Parameter::new(0.0, min, max); // base tracked from the slot each frame
        p.source = src;
        p.amount = amount;
        p.smoothing = smoothing.clamp(0.0, 0.999);
        p.invert = invert;
        p.curve = curve_from(&curve);
        s.compositor.set_control_mod(id, i, Some(p));
    });
}

fn curve_from(s: &str) -> Curve {
    match s {
        "exp" => Curve::Exp,
        "log" => Curve::Log,
        "scurve" => Curve::SCurve,
        _ => Curve::Linear,
    }
}

/// Update `iMouse` = (x, y, z, w). JS computes Shadertoy semantics (xy = current
/// position while pressed; z/w = click origin with sign) in canvas pixels,
/// bottom-left origin, DPR-scaled.
#[wasm_bindgen]
pub fn set_mouse(x: f32, y: f32, z: f32, w: f32) {
    with_state(|s| s.mouse = [x, y, z, w]);
}

// --- Visual clock ---------------------------------------------------------

#[wasm_bindgen]
pub fn set_time_scale(scale: f32) {
    with_state(|s| s.clock.set_scale(scale));
}
#[wasm_bindgen]
pub fn set_paused(paused: bool) {
    with_state(|s| s.clock.set_paused(paused));
}
#[wasm_bindgen]
pub fn reset_time() {
    with_state(|s| {
        s.clock.reset();
        s.frame = 0;
    });
}

// --- Tempo ----------------------------------------------------------------

#[wasm_bindgen]
pub fn tempo_tap() {
    with_state(|s| s.tempo.tap(js_sys::Date::now() / 1000.0));
}
#[wasm_bindgen]
pub fn tempo_set_bpm(bpm: f32) {
    with_state(|s| s.tempo.set_manual_bpm(bpm));
}
#[wasm_bindgen]
pub fn tempo_set_manual(manual: bool) {
    with_state(|s| s.tempo.set_manual(manual));
}
#[wasm_bindgen]
pub fn tempo_half() {
    with_state(|s| s.tempo.half_time());
}
#[wasm_bindgen]
pub fn tempo_double() {
    with_state(|s| s.tempo.double_time());
}
#[wasm_bindgen]
pub fn tempo_reset_phase() {
    with_state(|s| s.tempo.reset_phase());
}
#[wasm_bindgen]
pub fn tempo_set_subdivision(sub: f32) {
    with_state(|s| s.tempo.set_subdivision(sub));
}

// --- LFO bank -------------------------------------------------------------

/// Configure one of the 4 LFOs. `wave`: 0 sine, 1 triangle, 2 saw, 3 square.
#[wasm_bindgen]
pub fn set_lfo(index: u32, wave: u8, rate: f32, tempo_sync: bool, mult: f32) {
    with_state(|s| {
        let i = index as usize;
        if i >= s.lfos.len() {
            return;
        }
        let lfo = &mut s.lfos[i];
        lfo.wave = match wave {
            1 => LfoWave::Triangle,
            2 => LfoWave::Saw,
            3 => LfoWave::Square,
            _ => LfoWave::Sine,
        };
        lfo.rate_hz = rate.max(0.0);
        lfo.tempo_sync = tempo_sync;
        lfo.mult = mult.max(0.0001);
    });
}

/// Minimal JSON string escaper for diagnostic messages.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// --- Overlay (selected waveform/spectrum layer) ---------------------------

/// Configure the selected overlay layer. `mode`: 0 oscilloscope, 1 mirrored,
/// 2 spectrum bars, 3 circular, 4 radial, 5 Lissajous. `channel`: 0 L, 1 R,
/// 2 mono (Lissajous always uses real L/R).
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn set_overlay(mode: u8, channel: u8, r: f32, g: f32, b: f32, opacity: f32, scale: f32, thickness: f32, rotation: f32, points: f32, log_freq: bool) {
    with_state(|s| {
        let Some(id) = s.compositor.selected() else { return };
        s.compositor.set_overlay_cfg(
            id,
            OverlayConfig { mode, channel, color: [r, g, b, opacity], scale, thickness, rotation, points, log_freq },
        );
    });
}

// --- Layer stack ----------------------------------------------------------

/// Add a layer. `kind`: 0 Milkdrop, 1 Shader, 2 Waveform, 3 Spectrum. Returns the
/// new layer id, or -1 if rejected (e.g. a second Milkdrop, or a limit hit).
#[wasm_bindgen]
pub fn add_layer(kind: u8) -> f64 {
    with_state(|s| {
        let k = match kind {
            1 => LayerKind::Shader,
            2 => LayerKind::Waveform,
            3 => LayerKind::Spectrum,
            _ => LayerKind::Milkdrop,
        };
        s.compositor.add_layer(&s.ctx, k).map(|id| id as f64).unwrap_or(-1.0)
    })
    .unwrap_or(-1.0)
}

#[wasm_bindgen]
pub fn remove_layer(id: f64) {
    with_state(|s| s.compositor.remove_layer(id as u64));
}

#[wasm_bindgen]
pub fn duplicate_layer(id: f64) -> f64 {
    with_state(|s| s.compositor.duplicate_layer(&s.ctx, id as u64).map(|i| i as f64).unwrap_or(-1.0)).unwrap_or(-1.0)
}

#[wasm_bindgen]
pub fn move_layer(id: f64, up: bool) {
    with_state(|s| s.compositor.move_layer(id as u64, up));
}

#[wasm_bindgen]
pub fn select_layer(id: f64) {
    with_state(|s| s.compositor.set_selected(id as u64));
}

#[wasm_bindgen]
pub fn set_layer_enabled(id: f64, enabled: bool) {
    with_state(|s| s.compositor.set_enabled(id as u64, enabled));
}

#[wasm_bindgen]
pub fn set_layer_visible(id: f64, visible: bool) {
    with_state(|s| s.compositor.set_visible(id as u64, visible));
}

#[wasm_bindgen]
pub fn set_layer_opacity(id: f64, opacity: f32) {
    with_state(|s| s.compositor.set_opacity(id as u64, opacity));
}

#[wasm_bindgen]
pub fn set_layer_blend(id: f64, blend: u32) {
    with_state(|s| s.compositor.set_blend(id as u64, blend));
}

#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn set_layer_transform(id: f64, px: f32, py: f32, sx: f32, sy: f32, rot: f32) {
    with_state(|s| s.compositor.set_transform(id as u64, px, py, sx, sy, rot));
}

#[wasm_bindgen]
pub fn rename_layer(id: f64, name: String) {
    with_state(|s| s.compositor.rename_layer(id as u64, name));
}

#[wasm_bindgen]
pub fn layers_json() -> String {
    with_state(|s| s.compositor.layers_json()).unwrap_or_else(|| "[]".into())
}

#[wasm_bindgen]
pub fn selected_controls_json() -> String {
    with_state(|s| s.compositor.selected_controls_json()).unwrap_or_else(|| "{}".into())
}

// --- Scenes ---------------------------------------------------------------

#[wasm_bindgen]
pub fn export_scene() -> String {
    with_state(|s| {
        let scene = s.compositor.export_scene(s.clock.scale(), s.clock.paused(), s.tempo.bpm(), s.tempo.manual(), 1.0);
        pm_scene::to_json(&scene)
    })
    .unwrap_or_else(|| "{}".into())
}

/// Import a scene transactionally: parse + validate first, and only swap the
/// live stack in on success — a bad import preserves the current scene.
#[wasm_bindgen]
pub fn import_scene(json: String) -> String {
    with_state(|s| match pm_scene::parse_scene(&json) {
        Ok(scene) => {
            s.compositor.import_scene(&s.ctx, &scene);
            s.clock.set_scale(scene.speed);
            s.clock.set_paused(scene.paused);
            if scene.tempo_manual {
                s.tempo.set_manual_bpm(scene.bpm);
            } else {
                s.tempo.set_manual(false);
            }
            "{\"ok\":true}".to_string()
        }
        Err(e) => format!("{{\"ok\":false,\"error\":{}}}", json_string(&e)),
    })
    .unwrap_or_else(|| "{\"ok\":false,\"error\":\"app not ready\"}".into())
}

#[wasm_bindgen]
pub fn reset_scene() {
    with_state(|s| s.compositor.load_default(&s.ctx));
}

fn request_animation_frame(f: &Closure<dyn FnMut()>) {
    web_sys::window()
        .expect("no window")
        .request_animation_frame(f.as_ref().unchecked_ref())
        .expect("request_animation_frame failed");
}
