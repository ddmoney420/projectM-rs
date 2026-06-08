//! Port of `MilkdropPreset/MotionVectors` — the grid of short line segments
//! that visualises the warp's optical flow.
//!
//! A grid of `mv_x` × `mv_y` points (clamped to 64×48). For each point, a line
//! is drawn from its position toward where that point was sampled from last
//! frame — looked up in the warp motion-field texture (the warped UV the warp
//! pass wrote). The vertex shader is a port of projectM's
//! `PresetMotionVectorsVertexShader`, including the `length_multiplier` /
//! `minimum_length` trail-length handling.

use pm_preset::PresetState;
use pm_render::wgpu;
use pm_render::{GpuContext, Texture, TARGET_FORMAT};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MvUniform {
    color: [f32; 4],
    length_multiplier: f32,
    minimum_length: f32,
    _pad: [f32; 2],
}

const MV_WGSL: &str = r#"
struct U {
    color: vec4<f32>,
    length_multiplier: f32,
    minimum_length: f32,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var warp_coords: texture_2d<f32>;
@group(0) @binding(2) var warp_smp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32, @location(0) vertex_position: vec2<f32>) -> VsOut {
    // Grid positions are texture coordinates (0..1).
    var pos = vertex_position;

    if (vid % 2u == 1u) {
        // The line's far end follows the motion field written by the warp pass.
        let old_uv = textureSampleLevel(warp_coords, warp_smp, vec2<f32>(pos.x, 1.0 - pos.y), 0.0).xy;
        var dist = (old_uv - pos) * u.length_multiplier;
        let len = length(dist);
        if (len > u.minimum_length) {
            // already long enough
        } else if (len > 0.00000001) {
            dist = dist * (u.minimum_length / len);
        } else {
            dist = vec2<f32>(u.minimum_length);
        }
        pos += dist;
    }

    // 0..1 -> -1..1, flip Y (drawn top-down).
    pos = pos * 2.0 - 1.0;
    pos.y = -pos.y;

    var out: VsOut;
    out.clip = vec4<f32>(pos, 0.0, 1.0);
    out.color = u.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

pub struct MotionVectors {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    vertex_buf: wgpu::Buffer,
    capacity: usize,
}

impl MotionVectors {
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("motion vectors bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("motion vectors shader"),
            source: wgpu::ShaderSource::Wgsl(MV_WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("motion vectors layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("motion vectors pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("motion vectors sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("motion vectors uniform"),
            size: std::mem::size_of::<MvUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("motion vectors vertices"),
            size: 64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        MotionVectors { pipeline, bind_group_layout, sampler, uniform_buf, vertex_buf, capacity: 0 }
    }

    /// Draw the motion-vector grid into `target`, sampling the warp `motion`
    /// field. No-op when the vectors are invisible or the grid is empty.
    pub fn draw(&mut self, ctx: &GpuContext, target: &wgpu::TextureView, motion: &Texture, state: &PresetState) {
        if state.mv_a < 0.0001 {
            return;
        }
        let verts = grid_lines(state);
        if verts.is_empty() {
            return;
        }

        let inv_w = 1.25 / state.frame.viewport_width.max(1) as f32;
        let inv_h = 1.25 / state.frame.viewport_height.max(1) as f32;
        let minimum_length = (inv_w * inv_w + inv_h * inv_h).sqrt();
        let uniform = MvUniform {
            color: [state.mv_r, state.mv_g, state.mv_b, state.mv_a],
            length_multiplier: state.mv_l,
            minimum_length,
            _pad: [0.0; 2],
        };
        ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));

        let bytes: &[u8] = bytemuck::cast_slice(&verts);
        if verts.len() > self.capacity {
            self.vertex_buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("motion vectors vertices"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity = verts.len();
        }
        ctx.queue.write_buffer(&self.vertex_buf, 0, bytes);

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("motion vectors bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&motion.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("motion vectors encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("motion vectors pass"),
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
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
            pass.draw(0..verts.len() as u32, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}

/// Build the grid line vertices (two coincident points per line; the vertex
/// shader displaces the second). Port of `MotionVectors::Draw`'s loops.
fn grid_lines(state: &PresetState) -> Vec<[f32; 2]> {
    let mut count_x = state.mv_x as i32;
    let mut count_y = state.mv_y as i32;
    if count_x <= 0 || count_y <= 0 {
        return Vec::new();
    }
    let mut divert_x = state.mv_x - count_x as f32;
    let mut divert_y = state.mv_y - count_y as f32;
    if count_x > 64 {
        count_x = 64;
        divert_x = 0.0;
    }
    if count_y > 48 {
        count_y = 48;
        divert_y = 0.0;
    }
    let divert_x2 = state.mv_dx;
    let divert_y2 = state.mv_dy;
    divert_x = divert_x.clamp(0.0, 1.0);
    divert_y = divert_y.clamp(0.0, 1.0);

    let mut verts = Vec::new();
    for y in 0..count_y {
        let pos_y = (y as f32 + 0.25) / (count_y as f32 + divert_y + 0.25 - 1.0) - divert_y2;
        if pos_y <= 0.0001 || pos_y >= 0.9999 {
            continue;
        }
        for x in 0..count_x {
            let pos_x = (x as f32 + 0.25) / (count_x as f32 + divert_x + 0.25 - 1.0) + divert_x2;
            if pos_x <= 0.0001 || pos_x >= 0.9999 {
                continue;
            }
            // Two coincident vertices; the shader moves the odd one along the flow.
            verts.push([pos_x, pos_y]);
            verts.push([pos_x, pos_y]);
        }
    }
    verts
}
