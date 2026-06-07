//! Fullscreen rendering helpers.
//!
//! A great deal of projectM's rendering is "run a fragment shader over the
//! whole target" — the final composite, blur stages, video echo, transitions.
//! [`FullscreenShader`] wraps that pattern: it supplies a standard fullscreen
//! vertex stage and takes a caller-provided fragment shader.

/// Standard fullscreen vertex stage. Emits a single oversized triangle and a
/// `uv` in `[0,1]`. The caller's fragment shader must be named `fs_main` with
/// the signature `fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32>`.
pub const FULLSCREEN_VERTEX_WGSL: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = corners[vid];
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    // Map clip space to UV; flip Y so (0,0) is top-left like image space.
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 1.0 - (p.y * 0.5 + 0.5));
    return out;
}
"#;

/// A render pipeline that draws a caller-supplied fragment shader fullscreen.
pub struct FullscreenShader {
    pipeline: wgpu::RenderPipeline,
}

impl FullscreenShader {
    /// Build a pipeline for the given target `format`. `fragment_wgsl` must
    /// define `fs_main` (see [`FULLSCREEN_VERTEX_WGSL`]); it is concatenated
    /// after the built-in vertex stage so both share one module.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, fragment_wgsl: &str) -> Self {
        let source = format!("{FULLSCREEN_VERTEX_WGSL}\n{fragment_wgsl}");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fullscreen shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fullscreen pipeline layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fullscreen pipeline"),
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

        FullscreenShader { pipeline }
    }

    /// Encode a draw of the fullscreen triangle into `target`. If `clear` is
    /// `Some`, the target is cleared to that color first; otherwise its existing
    /// contents are loaded.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clear: Option<wgpu::Color>,
    ) {
        let load = match clear {
            Some(c) => wgpu::LoadOp::Clear(c),
            None => wgpu::LoadOp::Load,
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fullscreen pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.draw(0..3, 0..1);
    }
}

/// Encode a render pass that only clears `target` to `color`.
pub fn clear(encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, color: wgpu::Color) {
    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(color),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
}
