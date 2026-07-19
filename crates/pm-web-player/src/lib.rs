//! `pm-web` — WebAssembly entry point for the projectM-rs engine.
//!
//! Browser counterpart to `pm-app`: it drives the same engine
//! (`WarpEngine::render_frame`) but swaps the platform edges — an
//! `HtmlCanvasElement` surface instead of a winit window, async device
//! requests instead of `pollster::block_on`, and audio pushed in from Web
//! Audio instead of cpal.
//!
//! The public API is a **handle** ([`PmEngine`]) that the host (a JS/React app)
//! owns and drives explicitly: create once, `load_preset`, then call `render`
//! from the host's own `requestAnimationFrame` loop. The host controls the
//! lifecycle — there is no internal forever-loop — so teardown is just dropping
//! the handle (`free()`), which cancels nothing of its own and leaves no timers
//! running.
//!
//! This crate is **wasm-only** — it builds a canvas-backed wgpu surface, which
//! only exists on `wasm32`. On other targets it compiles to an empty crate so
//! it never breaks a native workspace build.
#![cfg(target_arch = "wasm32")]

use pm_audio::{WAVEFORM_SAMPLES, PCM};
use pm_core::{PresetPlayer, WarpEngine};
use pm_preset::Preset;
use pm_render::wgpu;
use pm_render::GpuContext;
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

/// A live visualizer bound to one canvas. Owned and driven by the host:
///
/// ```text
/// const engine = await PmEngine.create(canvas, w, h);
/// engine.load_preset(milkText);          // returns false on parse failure
/// function frame(tMs) {
///   engine.push_audio(analyserSamples);  // optional, each frame
///   engine.render(tMs / 1000);
///   raf = requestAnimationFrame(frame);  // host owns the loop
/// }
/// // teardown: cancelAnimationFrame(raf); engine.free();
/// ```
#[wasm_bindgen]
pub struct PmEngine {
    ctx: GpuContext,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    blit: Blit,
    /// Drives the current preset and (during a `transition_to_preset` crossfade)
    /// the outgoing one, blending their outputs. `None` until the first preset
    /// is loaded.
    player: Option<PresetPlayer>,
    /// Kept so the player/engine can be rebuilt at a new resolution on `resize`.
    preset_text: Option<String>,
    pcm: PCM,
    /// Latest mono time-domain samples pushed from JS; empty → synthetic signal.
    live_audio: Vec<f32>,
    width: u32,
    height: u32,
    frame: i32,
}

#[wasm_bindgen]
impl PmEngine {
    /// Initialise wgpu on `canvas` (WebGPU, or WebGL2 fallback). No preset is
    /// loaded yet — call [`PmEngine::load_preset`] before [`PmEngine::render`].
    pub async fn create(
        canvas: web_sys::HtmlCanvasElement,
        width: u32,
        height: u32,
    ) -> Result<PmEngine, JsValue> {
        let width = width.max(1);
        let height = height.max(1);

        log::info!("pm-web: create: instance…");
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
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
        log::info!("pm-web: create: adapter ok ({:?})", adapter.get_info().backend);

        // WebGL2 caps are lower than native; request limits the surface supports
        // so the device request doesn't over-reach on browsers without WebGPU.
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
        let blit = Blit::new(&ctx, surface_format);
        log::info!("pm-web: create: ready ({width}x{height}) on {surface_format:?}");

        Ok(PmEngine {
            ctx,
            surface,
            config,
            blit,
            player: None,
            preset_text: None,
            pcm: PCM::new(),
            live_audio: Vec::new(),
            width,
            height,
            frame: 0,
        })
    }

    /// Parse and **hard-cut** to a `.milk` preset at the current resolution.
    /// Returns `false` (without disturbing the current preset) if the text fails
    /// to parse, so the host can skip it and advance. Behaviour is unchanged
    /// from before engine-level transitions existed: this is always an instant
    /// cut (with feedback inheritance, so feedback presets don't start black).
    pub fn load_preset(&mut self, text: String) -> bool {
        self.switch(text, 0.0)
    }

    /// Like [`PmEngine::load_preset`], but **crossfades** from the current preset
    /// over `duration_ms` milliseconds: both presets keep rendering live and
    /// their composited outputs are blended (an engine-level transition, not a
    /// frozen still). `duration_ms <= 0`, NaN, or any non-finite value falls
    /// back to a hard cut, so it never panics. The host chooses the duration —
    /// pm-web invents no default. Returns `false` on parse failure, leaving the
    /// current preset untouched.
    pub fn transition_to_preset(&mut self, text: String, duration_ms: f32) -> bool {
        let duration_secs = if duration_ms.is_finite() && duration_ms > 0.0 {
            duration_ms / 1000.0
        } else {
            0.0
        };
        self.switch(text, duration_secs)
    }

    /// Feed the latest mono, time-domain audio samples (e.g. from a Web Audio
    /// `AnalyserNode.getFloatTimeDomainData`). Optional; call once per frame.
    pub fn push_audio(&mut self, samples: &[f32]) {
        self.live_audio.clear();
        self.live_audio.extend_from_slice(samples);
    }

    /// Render one frame at `time_seconds` and present it. No-op until a preset
    /// is loaded. The host calls this from its own `requestAnimationFrame`.
    pub fn render(&mut self, time_seconds: f32) {
        if self.player.is_none() {
            return;
        }

        // Build this frame's audio: live samples if pushed, else the same
        // synthetic multi-tone signal pm-app's headless fallback uses.
        if !self.live_audio.is_empty() {
            let samples = std::mem::take(&mut self.live_audio);
            self.pcm.add_float(&samples, 1);
            self.live_audio = samples;
        } else {
            let mut samples = vec![0.0f32; WAVEFORM_SAMPLES * 2];
            for i in 0..WAVEFORM_SAMPLES {
                let p = i as f32 / WAVEFORM_SAMPLES as f32;
                let s = (p * 24.0 + time_seconds * 2.0).sin() * 0.4
                    + (p * 7.0 - time_seconds).sin() * 0.25
                    + (p * 53.0 + time_seconds * 0.5).sin() * 0.1;
                samples[i * 2] = s;
                samples[i * 2 + 1] = s * 0.8;
            }
            self.pcm.add_float(&samples, 2);
        }
        self.pcm.update_frame_audio_data(1.0 / 60.0, self.frame as u32);
        let audio = self.pcm.frame_audio_data();

        // Advance the preset(s) and, mid-transition, blend their outputs. The
        // PresetPlayer feeds the same audio to both engines, tracks per-preset
        // frame counters, and retires the outgoing engine when the crossfade
        // completes. (It swallows per-frame render errors internally.)
        self.player
            .as_mut()
            .unwrap()
            .render(&self.ctx, time_seconds, audio);

        let surf = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            // Outdated/Lost/Timeout/Occluded/Validation: nothing usable this
            // tick (e.g. canvas resized/backgrounded); retry next frame.
            other => {
                log::warn!("pm-web: surface unavailable this frame: {other:?}");
                return;
            }
        };
        let view = surf.texture.create_view(&wgpu::TextureViewDescriptor::default());
        // `output_texture()` is the blended target mid-transition, else the
        // current preset's display texture directly (no extra blend cost idle).
        self.blit.draw(
            &self.ctx,
            self.player.as_ref().unwrap().output_texture(),
            &view,
        );
        surf.present();
        self.frame += 1;
    }

    /// Resize the surface (and rebuild the engine at the new resolution so the
    /// internal render targets match). Call when the canvas's pixel size changes.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.config.width = self.width;
        self.config.height = self.height;
        self.surface.configure(&self.ctx.device, &self.config);

        // Rebuild the player at the new size from the current preset, if any.
        // Engines/output are sized at construction, so a resize mid-transition
        // rebuilds the player and DROPS any in-flight crossfade (it snaps to the
        // current preset at the new resolution) — an accepted simplification,
        // since resize is rare and not worth preserving a fade across.
        if let Some(text) = self.preset_text.clone() {
            if let Ok(preset) = Preset::load(&text) {
                let engine = WarpEngine::new(&self.ctx, preset, self.width, self.height);
                self.player =
                    Some(PresetPlayer::new(&self.ctx, engine, self.width, self.height, 0.0));
                self.frame = 0;
            }
        }
    }
}

impl PmEngine {
    /// Shared body for [`PmEngine::load_preset`] (hard cut, `duration_secs == 0`)
    /// and [`PmEngine::transition_to_preset`] (crossfade). Builds the incoming
    /// engine — inheriting the previous preset's last completed frame so
    /// feedback/transition presets don't start black (the beta.5 behaviour) —
    /// then switches the [`PresetPlayer`] with the given duration. On the very
    /// first preset there is nothing to inherit from or fade, so it just creates
    /// the player. Leaves the current preset untouched and returns `false` if
    /// the text fails to parse.
    fn switch(&mut self, text: String, duration_secs: f32) -> bool {
        let preset = match Preset::load(&text) {
            Ok(preset) => preset,
            Err(e) => {
                log::warn!("pm-web: preset parse failed: {e:?}");
                return false;
            }
        };
        // Build the incoming engine, inheriting feedback from the current one
        // (a GPU texture copy submitted before the borrow ends).
        let engine = match self.player.as_ref() {
            Some(player) => WarpEngine::new_inheriting(
                &self.ctx,
                preset,
                self.width,
                self.height,
                Some(player.current_engine()),
            ),
            None => WarpEngine::new(&self.ctx, preset, self.width, self.height),
        };
        match self.player.as_mut() {
            Some(player) => {
                player.set_duration(duration_secs);
                player.switch_to(engine);
            }
            None => {
                self.player = Some(PresetPlayer::new(
                    &self.ctx,
                    engine,
                    self.width,
                    self.height,
                    duration_secs,
                ));
            }
        }
        self.preset_text = Some(text);
        self.frame = 0;
        true
    }
}
