//! GPU warp pass on wgpu: the WGSL port of Milkdrop's warp shader plus the
//! current/previous-frame ping-pong that produces the feedback "flow".

use crate::warp_mesh::{WarpMesh, WarpVertex};
use bytemuck::{Pod, Zeroable};
use pm_render::wgpu;
use pm_render::{GpuContext, Texture, TARGET_FORMAT};

/// Per-frame warp parameters (computed from the preset state by the caller).
#[derive(Debug, Clone, Copy)]
pub struct WarpParams {
    /// `(aspectX, aspectY, invAspectX, invAspectY)`.
    pub aspect: [f32; 4],
    pub warp_factors: [f32; 4],
    pub texel_offset: [f32; 2],
    pub warp_time: f32,
    pub warp_scale_inverse: f32,
    pub decay: f32,
    /// `true` to wrap (repeat) the sampler, `false` to clamp to edge.
    pub wrap: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    aspect: [f32; 4],
    warp_factors: [f32; 4],
    texel_offset: [f32; 2],
    warp_time: f32,
    warp_scale_inverse: f32,
    decay: f32,
    _pad: [f32; 3],
}

const WARP_WGSL: &str = r#"
struct Uniforms {
    aspect: vec4<f32>,
    warp_factors: vec4<f32>,
    texel_offset: vec2<f32>,
    warp_time: f32,
    warp_scale_inverse: f32,
    decay: f32,
};
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var prev_tex: texture_2d<f32>;
@group(0) @binding(2) var prev_smp: sampler;

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) rad_ang: vec2<f32>,
    @location(2) transforms: vec4<f32>,
    @location(3) center: vec2<f32>,
    @location(4) distance: vec2<f32>,
    @location(5) stretch: vec2<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) decay: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let pos = in.pos;
    let radius = in.rad_ang.x;
    let zoom = in.transforms.x;
    let zoom_exp = in.transforms.y;
    let rot = in.transforms.z;
    let warp = in.transforms.w;
    let aspect_x = u.aspect.x;
    let aspect_y = u.aspect.y;
    let inv_aspect_x = u.aspect.z;
    let inv_aspect_y = u.aspect.w;

    var out: VsOut;
    // Orthographic projection ortho(-1,1,1,-1,...) reduces to a Y flip at z=0.
    out.clip = vec4<f32>(pos.x, -pos.y, 0.0, 1.0);

    let zoom2 = pow(zoom, pow(zoom_exp, radius * 2.0 - 1.0));
    let zoom2_inv = 1.0 / zoom2;

    var uu = pos.x * aspect_x * 0.5 * zoom2_inv + 0.5;
    var vv = pos.y * aspect_y * 0.5 * zoom2_inv + 0.5;

    // Stretch
    uu = (uu - in.center.x) / in.stretch.x + in.center.x;
    vv = (vv - in.center.y) / in.stretch.y + in.center.y;

    // Warp
    let wt = u.warp_time;
    let wsi = u.warp_scale_inverse;
    let wf = u.warp_factors;
    uu = uu + warp * 0.0035 * sin(wt * 0.333 + wsi * (pos.x * wf.x - pos.y * wf.w));
    vv = vv + warp * 0.0035 * cos(wt * 0.375 - wsi * (pos.x * wf.z + pos.y * wf.y));
    uu = uu + warp * 0.0035 * cos(wt * 0.753 - wsi * (pos.x * wf.y - pos.y * wf.z));
    vv = vv + warp * 0.0035 * sin(wt * 0.825 + wsi * (pos.x * wf.x + pos.y * wf.w));

    // Rotation about the warp center
    let u2 = uu - in.center.x;
    let v2 = vv - in.center.y;
    let cr = cos(rot);
    let sr = sin(rot);
    uu = u2 * cr - v2 * sr + in.center.x;
    vv = u2 * sr + v2 * cr + in.center.y;

    // Translation
    uu = uu - in.distance.x;
    vv = vv - in.distance.y;

    // Undo aspect-ratio fix
    uu = (uu - 0.5) * inv_aspect_x + 0.5;
    vv = (vv - 0.5) * inv_aspect_y + 0.5;

    // Texel alignment
    uu = uu + u.texel_offset.x;
    vv = vv + u.texel_offset.y;

    out.uv = vec2<f32>(uu, vv);
    out.decay = u.decay;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(prev_tex, prev_smp, in.uv);
    return vec4<f32>(c.rgb * in.decay, c.a);
}
"#;

pub struct WarpRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler_repeat: wgpu::Sampler,
    sampler_clamp: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    vertex_buf: wgpu::Buffer,
    vertex_capacity: usize,
    index_buf: wgpu::Buffer,
    index_count: u32,
    main: [Texture; 2],
    current: usize,
    width: u32,
    height: u32,
}

impl WarpRenderer {
    pub fn new(ctx: &GpuContext, width: u32, height: u32) -> Self {
        let device = &ctx.device;

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("warp bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("warp shader"),
            source: wgpu::ShaderSource::Wgsl(WARP_WGSL.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("warp pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("warp pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[WarpVertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: None,
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
        });

        let sampler_repeat = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("warp sampler (repeat)"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let sampler_clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("warp sampler (clamp)"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("warp uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Placeholder buffers; (re)sized on first frame.
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("warp vertices"),
            size: 64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("warp indices"),
            size: 64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let main = [
            Texture::new_render_target(device, "main[0]", width, height, TARGET_FORMAT),
            Texture::new_render_target(device, "main[1]", width, height, TARGET_FORMAT),
        ];

        WarpRenderer {
            pipeline,
            bind_group_layout,
            sampler_repeat,
            sampler_clamp,
            uniform_buf,
            vertex_buf,
            vertex_capacity: 0,
            index_buf,
            index_count: 0,
            main,
            current: 0,
            width,
            height,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The texture holding the latest frame.
    pub fn main_texture(&self) -> &Texture {
        &self.main[self.current]
    }

    /// View of the current frame, for drawing waveforms/shapes on top.
    pub fn current_view(&self) -> &wgpu::TextureView {
        &self.main[self.current].view
    }

    /// Upload an RGBA8 image into the current main texture (initial content).
    pub fn seed(&self, ctx: &GpuContext, rgba: &[u8]) {
        let tex = &self.main[self.current].texture;
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.width * 4),
                rows_per_image: Some(self.height),
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }

    /// Render one warp pass: sample the current frame through the warped mesh
    /// into the other buffer, then make it current.
    pub fn warp_frame(&mut self, ctx: &GpuContext, mesh: &WarpMesh, params: &WarpParams) {
        let device = &ctx.device;

        // Upload vertices (grow buffer if needed).
        let vertex_bytes: &[u8] = bytemuck::cast_slice(mesh.vertices());
        if mesh.vertices().len() > self.vertex_capacity {
            self.vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("warp vertices"),
                size: vertex_bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vertex_capacity = mesh.vertices().len();
        }
        ctx.queue.write_buffer(&self.vertex_buf, 0, vertex_bytes);

        // Upload indices (recreate if count changed).
        if mesh.index_count() != self.index_count {
            let index_bytes: &[u8] = bytemuck::cast_slice(mesh.indices());
            self.index_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("warp indices"),
                size: index_bytes.len() as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            ctx.queue.write_buffer(&self.index_buf, 0, index_bytes);
            self.index_count = mesh.index_count();
        }

        // Uniforms.
        let uniforms = Uniforms {
            aspect: params.aspect,
            warp_factors: params.warp_factors,
            texel_offset: params.texel_offset,
            warp_time: params.warp_time,
            warp_scale_inverse: params.warp_scale_inverse,
            decay: params.decay,
            _pad: [0.0; 3],
        };
        ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let source = self.current;
        let dest = 1 - self.current;
        let sampler = if params.wrap { &self.sampler_repeat } else { &self.sampler_clamp };

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("warp bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.main[source].view),
                },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        });

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("warp encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("warp pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.main[dest].view,
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
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
            pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));

        self.current = dest;
    }
}
