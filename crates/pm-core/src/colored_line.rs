//! Per-vertex-colored line/point renderer, used for custom waveforms (each
//! point has its own `r`/`g`/`b`/`a` from the per-point code).

use bytemuck::{Pod, Zeroable};
use pm_render::wgpu;
use pm_render::{GpuContext, TARGET_FORMAT};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    color: [f32; 4],
}

const WGSL: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) color: vec4<f32>) -> VsOut {
    var out: VsOut;
    out.pos = vec4<f32>(pos.x, -pos.y, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

pub struct ColoredLineRenderer {
    /// `[additive][dots]` pipelines.
    pipelines: [[wgpu::RenderPipeline; 2]; 2],
    vertex_buf: wgpu::Buffer,
    capacity: usize,
}

impl ColoredLineRenderer {
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("colored line shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("colored line layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let alpha = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };

        let make = |blend: wgpu::BlendState, topology: wgpu::PrimitiveTopology| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("colored line pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 8, shader_location: 1 },
                        ],
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
                primitive: wgpu::PrimitiveState { topology, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        use wgpu::PrimitiveTopology::{LineStrip, PointList};
        let pipelines = [
            [make(alpha, LineStrip), make(alpha, PointList)],
            [make(additive, LineStrip), make(additive, PointList)],
        ];

        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("colored line vertices"),
            size: 256,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        ColoredLineRenderer { pipelines, vertex_buf, capacity: 0 }
    }

    /// Draw `points` (with matching `colors`) into `target`.
    pub fn draw(
        &mut self,
        ctx: &GpuContext,
        target: &wgpu::TextureView,
        points: &[[f32; 2]],
        colors: &[[f32; 4]],
        additive: bool,
        dots: bool,
    ) {
        let n = points.len().min(colors.len());
        if n < 2 {
            return;
        }
        let verts: Vec<Vertex> = (0..n).map(|i| Vertex { pos: points[i], color: colors[i] }).collect();
        let bytes: &[u8] = bytemuck::cast_slice(&verts);
        if n > self.capacity {
            self.vertex_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("colored line vertices"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity = n;
        }
        ctx.queue.write_buffer(&self.vertex_buf, 0, bytes);

        let pipeline = &self.pipelines[additive as usize][dots as usize];

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("colored line encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("colored line pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
            pass.draw(0..n as u32, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
