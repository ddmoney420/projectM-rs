//! Renders **feedback-textured** custom shapes: a triangle-list fill that
//! samples a source texture (the warp feedback buffer) at per-vertex UVs and
//! multiplies by the per-vertex colour.
//!
//! This is the non-asset path of Milkdrop's textured shapes (`shapecode_N_tex`
//! / the `textured` per-frame flag): the shape stamps a zoomed/rotated copy of
//! the current image. External image-file textures are **not** supported here —
//! only the in-pipeline feedback texture is sampled.

use bytemuck::{Pod, Zeroable};
use pm_render::wgpu;
use pm_render::{GpuContext, TARGET_FORMAT};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    color: [f32; 4],
    uv: [f32; 2],
}

const WGSL: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) color: vec4<f32>, @location(2) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.pos = vec4<f32>(pos.x, -pos.y, 0.0, 1.0);
    out.color = color;
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = textureSample(src_tex, src_smp, in.uv);
    return t * in.color;
}
"#;

pub struct TexturedShapeRenderer {
    /// `[additive]` triangle-list pipelines.
    pipelines: [wgpu::RenderPipeline; 2],
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertex_buf: wgpu::Buffer,
    capacity: usize,
}

impl TexturedShapeRenderer {
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("textured shape shader"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("textured shape bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("textured shape layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let alpha = wgpu::BlendState::ALPHA_BLENDING;
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let make = |blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("textured shape pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x2],
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
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("textured shape sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("textured shape vertices"),
            size: 64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        TexturedShapeRenderer {
            pipelines: [make(alpha), make(additive)],
            bind_group_layout,
            sampler,
            vertex_buf,
            capacity: 0,
        }
    }

    /// Draw a textured triangle-list fill, sampling `source` at the given UVs.
    /// No-op if the inputs are malformed (fewer than 3 vertices).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        ctx: &GpuContext,
        target: &wgpu::TextureView,
        source: &wgpu::TextureView,
        points: &[[f32; 2]],
        colors: &[[f32; 4]],
        uvs: &[[f32; 2]],
        additive: bool,
    ) {
        let n = points.len().min(colors.len()).min(uvs.len());
        if n < 3 {
            return;
        }
        let verts: Vec<Vertex> =
            (0..n).map(|i| Vertex { pos: points[i], color: colors[i], uv: uvs[i] }).collect();
        let bytes: &[u8] = bytemuck::cast_slice(&verts);
        if n > self.capacity {
            self.vertex_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("textured shape vertices"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity = n;
        }
        ctx.queue.write_buffer(&self.vertex_buf, 0, bytes);

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("textured shape bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("textured shape encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("textured shape pass"),
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
            pass.set_pipeline(&self.pipelines[additive as usize]);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
            pass.draw(0..n as u32, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
