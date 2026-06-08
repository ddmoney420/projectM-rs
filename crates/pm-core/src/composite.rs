//! Port of the default `FinalComposite` / `VideoEcho` / `Filters` path: draws
//! the feedback buffer to the display target with the classic Milkdrop-1 effects
//! used when a preset has no composite shader.
//!
//! In one fragment: the animated bilinear hue gradient
//! ([`hue_shades`]/`ApplyHueShaderColors`), the **video echo** (blend a
//! zoomed+oriented copy of the frame, `VideoEcho`), the **gamma** brighten
//! (`gammaAdj`), and the **filters** brighten/darken/solarize/invert
//! (`Filters`). The multi-pass GL blend tricks of the original collapse to
//! closed-form colour math here (e.g. invert = `1-c`, darken = `c²`).

use crate::CompositeEffects;
use bytemuck::{Pod, Zeroable};
use pm_render::wgpu;
use pm_render::{GpuContext, Texture, TARGET_FORMAT};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct HueUniform {
    /// Four corner shades (rgb in xyz). Order: (1,1), (0,1), (1,0), (0,0).
    shades: [[f32; 4]; 4],
    /// Video echo: `(zoom, alpha, orientation, gammaAdj)`.
    echo: [f32; 4],
    /// Filter flags as 0/1: `(brighten, darken, solarize, invert)`.
    filters: [f32; 4],
}

const COMPOSITE_WGSL: &str = r#"
struct U {
    shades: array<vec4<f32>, 4>,
    echo: vec4<f32>,
    filters: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_smp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0),
    );
    let p = corners[vid];
    var out: VsOut;
    out.clip = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, p.y * 0.5 + 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let x = uv.x;
    let y = uv.y;
    let hue = u.shades[0].rgb * (x * y)
            + u.shades[1].rgb * ((1.0 - x) * y)
            + u.shades[2].rgb * (x * (1.0 - y))
            + u.shades[3].rgb * ((1.0 - x) * (1.0 - y));

    let main = textureSample(src_tex, src_smp, uv).rgb;
    var color = main;

    // Video echo: blend a zoomed + (optionally flipped) copy of the frame.
    let alpha = u.echo.y;
    if (alpha > 0.001) {
        let zoom = max(u.echo.x, 0.0001);
        var euv = (uv - vec2<f32>(0.5)) / zoom + vec2<f32>(0.5);
        let orient = u.echo.z;
        if (orient % 2.0 >= 1.0) { euv.x = 1.0 - euv.x; }
        if (orient >= 2.0) { euv.y = 1.0 - euv.y; }
        let echo = textureSample(src_tex, src_smp, euv).rgb;
        color = main * (1.0 - alpha) + echo * alpha;
    }

    // Hue tint and gamma brighten.
    color = color * hue * u.echo.w;

    // Filters (applied in Milkdrop's order: brighten, darken, solarize, invert).
    let one = vec3<f32>(1.0);
    if (u.filters.x > 0.5) { color = one - (one - color) * (one - color); }
    if (u.filters.y > 0.5) { color = color * color; }
    if (u.filters.z > 0.5) { color = 2.0 * color * (one - color); }
    if (u.filters.w > 0.5) { color = one - color; }

    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

pub struct CompositeRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    output: Texture,
}

impl CompositeRenderer {
    pub fn new(ctx: &GpuContext, width: u32, height: u32) -> Self {
        let device = &ctx.device;

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
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
            label: Some("composite shader"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite pipeline"),
            layout: Some(&layout),
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
            label: Some("composite sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("composite hue"),
            size: std::mem::size_of::<HueUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let output = Texture::new_render_target(device, "composite output", width, height, TARGET_FORMAT);

        CompositeRenderer { pipeline, bind_group_layout, sampler, uniform_buf, output }
    }

    pub fn output(&self) -> &Texture {
        &self.output
    }

    /// Draw `source` (the feedback buffer) to the output with the hue tint,
    /// video echo, gamma and filters.
    pub fn draw(&self, ctx: &GpuContext, source: &Texture, shades: [[f32; 4]; 4], fx: CompositeEffects) {
        let uniform = HueUniform {
            shades,
            echo: [fx.echo_zoom, fx.echo_alpha, fx.echo_orientation as f32, fx.gamma],
            filters: [
                fx.brighten as u8 as f32,
                fx.darken as u8 as f32,
                fx.solarize as u8 as f32,
                fx.invert as u8 as f32,
            ],
        };
        ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&source.view),
                },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("composite encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.output.view,
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
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}

/// Compute the four animated corner hue shades (port of `ApplyHueShaderColors`).
pub fn hue_shades(time: f32, hue_offsets: [f32; 4]) -> [[f32; 4]; 4] {
    let mut shades = [[1.0f32; 4]; 4];
    for (i, shade) in shades.iter_mut().enumerate() {
        let fi = i as f32;
        let mut s = [
            0.6 + 0.3 * (time * 30.0 * 0.0143 + 3.0 + fi * 21.0 + hue_offsets[3]).sin(),
            0.6 + 0.3 * (time * 30.0 * 0.0107 + 1.0 + fi * 13.0 + hue_offsets[1]).sin(),
            0.6 + 0.3 * (time * 30.0 * 0.0129 + 6.0 + fi * 9.0 + hue_offsets[2]).sin(),
        ];
        let max = s[0].max(s[1]).max(s[2]).max(1e-4);
        for c in &mut s {
            *c = 0.5 + 0.5 * (*c / max);
        }
        *shade = [s[0], s[1], s[2], 1.0];
    }
    shades
}
