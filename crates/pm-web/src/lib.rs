//! `pm-web` — the browser/WASM frontend adapter for projectM-rs.
//!
//! This crate owns only the browser-specific seams (canvas + async WebGPU init,
//! render loop, Web Audio bridge, storage, URL import/export). The visualizer
//! engine itself lives in the platform-neutral `pm-*` crates and is wired in
//! from Phase 2 onward.
//!
//! The whole crate is `cfg`-gated to `wasm32`, so a native `cargo build
//! --workspace` compiles it to an empty library and `pm-app` is unaffected.
//!
//! Phase 1 scope (this file): detect WebGPU, initialize a device/surface from a
//! `<canvas>`, and drive a `requestAnimationFrame` loop that renders one known
//! visual (an animated gradient) — proving the canvas → WebGPU → present path,
//! resize handling, and surface-lost recovery.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

/// Fullscreen-triangle vertex + animated gradient fragment. `u.time` advances a
/// fixed 1/60 s per frame so the motion is deterministic and independent of the
/// browser clock (matching the engine's fixed-timestep philosophy).
const GRADIENT_WGSL: &str = r#"
struct Uniforms { time: f32, width: f32, height: f32, _pad: f32 };
@group(0) @binding(0) var<uniform> u: Uniforms;

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    return vec4<f32>(p[idx], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag.xy / vec2<f32>(u.width, u.height);
    let col = 0.5 + 0.5 * cos(u.time + vec3<f32>(uv.x, uv.y, uv.x) * 3.0 + vec3<f32>(0.0, 2.0, 4.0));
    return vec4<f32>(col, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    time: f32,
    width: f32,
    height: f32,
    _pad: f32,
}

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
    surface.configure(&device, &config);

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("pm-web gradient"),
        source: wgpu::ShaderSource::Wgsl(GRADIENT_WGSL.into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("pm-web bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });

    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pm-web uniforms"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pm-web bind group"),
        layout: &bind_group_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pm-web layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("pm-web pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let state = Rc::new(RefCell::new(State {
        surface,
        device,
        queue,
        config,
        pipeline,
        uniform_buf,
        bind_group,
        canvas,
        time: 0.0,
    }));

    log::info!("pm-web: WebGPU initialized ({width}x{height}); starting render loop");

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

/// Everything the render loop needs to draw and recover across frames.
struct State {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    canvas: HtmlCanvasElement,
    time: f32,
}

impl State {
    fn render(&mut self) {
        // Track the canvas backing-store size (the JS shell applies DPR), and
        // reconfigure the surface on any change so resizes are handled here.
        let w = self.canvas.width().max(1);
        let h = self.canvas.height().max(1);
        if w != self.config.width || h != self.config.height {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
        }

        self.time += 1.0 / 60.0;
        self.queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&Uniforms {
                time: self.time,
                width: self.config.width as f32,
                height: self.config.height as f32,
                _pad: 0.0,
            }),
        );

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            // Lost/Outdated: reconfigure and skip this frame; the next tick draws.
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            // Timeout / Occluded / Validation: skip this frame.
            _ => return,
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("pm-web frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pm-web pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }
}

fn request_animation_frame(f: &Closure<dyn FnMut()>) {
    web_sys::window()
        .expect("no window")
        .request_animation_frame(f.as_ref().unchecked_ref())
        .expect("request_animation_frame failed");
}
