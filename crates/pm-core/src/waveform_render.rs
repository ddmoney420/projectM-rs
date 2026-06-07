//! GPU line renderer for waveforms — draws the generated points into the
//! feedback buffer with alpha or additive blending (so the trails flow).

use bytemuck::{Pod, Zeroable};
use pm_render::wgpu;
use pm_render::{GpuContext, TARGET_FORMAT};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ColorUniform {
    color: [f32; 4],
}

const WAVE_WGSL: &str = r#"
struct U { color: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;

@vertex
fn vs_main(@location(0) pos: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(pos.x, -pos.y, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return u.color;
}
"#;

pub struct WaveformRenderer {
    pipeline_alpha: wgpu::RenderPipeline,
    pipeline_additive: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    vertex_buf: wgpu::Buffer,
    vertex_capacity: usize,
}

impl WaveformRenderer {
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waveform bgl"),
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

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waveform shader"),
            source: wgpu::ShaderSource::Wgsl(WAVE_WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waveform layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let make_pipeline = |blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("waveform pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TARGET_FORMAT,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let alpha_blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        let additive_blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };

        let pipeline_alpha = make_pipeline(alpha_blend);
        let pipeline_additive = make_pipeline(additive_blend);

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waveform color"),
            size: std::mem::size_of::<ColorUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waveform vertices"),
            size: 64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        WaveformRenderer {
            pipeline_alpha,
            pipeline_additive,
            bind_group_layout,
            uniform_buf,
            vertex_buf,
            vertex_capacity: 0,
        }
    }

    /// Draw a line strip of `points` into `target` with the given RGBA `color`.
    pub fn draw(
        &mut self,
        ctx: &GpuContext,
        target: &wgpu::TextureView,
        points: &[[f32; 2]],
        color: [f32; 4],
        additive: bool,
        is_loop: bool,
    ) {
        if points.len() < 2 {
            return;
        }
        let device = &ctx.device;

        // For a closed loop, repeat the first point to close the strip.
        let mut data: Vec<[f32; 2]> = points.to_vec();
        if is_loop {
            data.push(points[0]);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&data);
        if data.len() > self.vertex_capacity {
            self.vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("waveform vertices"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vertex_capacity = data.len();
        }
        ctx.queue.write_buffer(&self.vertex_buf, 0, bytes);
        ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&ColorUniform { color }));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waveform bg"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() }],
        });

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("waveform encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waveform pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    // Load: draw on top of the warped frame.
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(if additive { &self.pipeline_additive } else { &self.pipeline_alpha });
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
            pass.draw(0..data.len() as u32, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
