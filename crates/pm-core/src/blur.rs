//! Milkdrop's blur chain (`sampler_blur1` / `blur2` / `blur3`).
//!
//! Presets read `GetBlur1/2/3(uv)` for soft, downsampled copies of the frame —
//! glow, bloom, smear, edge detection. Milkdrop builds them with a separable
//! Gaussian at progressively lower resolution: `blur1` from the current feedback
//! buffer, `blur2` from `blur1`, `blur3` from `blur2`. We do the same with two
//! passes (horizontal then vertical) per level.
//!
//! The `GetBlurN` macros denormalize by `*_c5/_c6` (the `blurN_min/max` range),
//! which defaults to identity, so the textures store the blurred colour directly.

use pm_render::wgpu;
use pm_render::{GpuContext, Texture, TARGET_FORMAT};

/// Each blur level renders at this fraction of the previous level's size.
const DOWNSCALE: u32 = 2;
/// Gaussian taps either side of centre; sample reach in source texels per step.
const SPREAD: f32 = 2.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurDir {
    /// Texel step along the blur axis (`1/size * spread`, 0 on the other axis).
    direction: [f32; 2],
    _pad: [f32; 2],
}

struct Level {
    temp: Texture,
    output: Texture,
    height: u32,
}

/// The three-level Gaussian blur chain, rebuilt each frame from the feedback.
pub struct Blur {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    dir_h: wgpu::Buffer,
    dir_v: [wgpu::Buffer; 3],
    levels: [Level; 3],
}

impl Blur {
    pub fn new(ctx: &GpuContext, width: u32, height: u32) -> Self {
        let device = &ctx.device;

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur shader"),
            source: wgpu::ShaderSource::Wgsl(BLUR_WGSL.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blur pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blur pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
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

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blur sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let mk_level = |i: usize, w: u32, h: u32| Level {
            temp: Texture::new_render_target(device, format!("blur{}_temp", i + 1), w, h, TARGET_FORMAT),
            output: Texture::new_render_target(device, format!("sampler_blur{}", i + 1), w, h, TARGET_FORMAT),
            height: h,
        };

        let mut w = width;
        let mut h = height;
        let mut levels = Vec::with_capacity(3);
        for i in 0..3 {
            w = (w / DOWNSCALE).max(1);
            h = (h / DOWNSCALE).max(1);
            levels.push(mk_level(i, w, h));
        }
        let levels: [Level; 3] = levels.try_into().unwrap_or_else(|_| unreachable!());

        // Horizontal step is shared (uses the source width); vertical steps use
        // each level's own height. Direction is in *destination* UV space, so
        // 1/size gives one destination texel.
        let dir = |dx: f32, dy: f32| {
            let buf = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("blur direction"),
                size: std::mem::size_of::<BlurDir>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            ctx.queue.write_buffer(&buf, 0, bytemuck::bytes_of(&BlurDir { direction: [dx, dy], _pad: [0.0; 2] }));
            buf
        };
        let dir_h = dir(SPREAD / width.max(1) as f32, 0.0);
        let dir_v = [
            dir(0.0, SPREAD / levels[0].height.max(1) as f32),
            dir(0.0, SPREAD / levels[1].height.max(1) as f32),
            dir(0.0, SPREAD / levels[2].height.max(1) as f32),
        ];

        Blur { pipeline, layout, sampler, dir_h, dir_v, levels }
    }

    /// Rebuild all three blur levels from `source` (the current feedback).
    pub fn compute(&self, ctx: &GpuContext, source: &Texture) {
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("blur encoder") });
        for i in 0..3 {
            let input = if i == 0 { source } else { &self.levels[i - 1].output };
            // Horizontal pass: input -> temp.
            self.pass(ctx, &mut encoder, &input.view, &self.levels[i].temp.view, &self.dir_h);
            // Vertical pass: temp -> output.
            self.pass(ctx, &mut encoder, &self.levels[i].temp.view, &self.levels[i].output.view, &self.dir_v[i]);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }

    fn pass(
        &self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        input: &wgpu::TextureView,
        target: &wgpu::TextureView,
        dir: &wgpu::Buffer,
    ) {
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blur bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: dir.as_entire_binding() },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blur pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// The blur texture bound to `name` (`sampler_blur1` .. `sampler_blur3`).
    pub fn get(&self, name: &str) -> Option<&Texture> {
        match name {
            "sampler_blur1" => Some(&self.levels[0].output),
            "sampler_blur2" => Some(&self.levels[1].output),
            "sampler_blur3" => Some(&self.levels[2].output),
            _ => None,
        }
    }
}

const BLUR_WGSL: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0),
    );
    let p = corners[vid];
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 1.0 - (p.y * 0.5 + 0.5));
    return out;
}

struct BlurDir { direction: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> u: BlurDir;

// 9-tap Gaussian (sigma ~2): centre + 4 symmetric taps.
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let w = array<f32, 5>(0.2270270270, 0.1945945946, 0.1216216216, 0.0540540541, 0.0162162162);
    let d = u.direction;
    var acc = textureSample(src, samp, in.uv).rgb * w[0];
    for (var i = 1; i < 5; i = i + 1) {
        let o = d * f32(i);
        acc += textureSample(src, samp, in.uv + o).rgb * w[i];
        acc += textureSample(src, samp, in.uv - o).rgb * w[i];
    }
    return vec4<f32>(acc, 1.0);
}
"#;
