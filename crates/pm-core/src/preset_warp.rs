//! Custom warp shader pipeline: the preset's warp shader runs as the fragment
//! over the warp mesh. The vertex stage (same warp UV computation as the default
//! warp) feeds the transpiled `PS` the warped sampling UV; the fragment samples
//! `sampler_main` (the feedback buffer) and runs the preset's color math.

use crate::warp_mesh::WarpVertex;
use pm_preset::{md_load_uniforms, md_uniforms_struct, WarpShaderParts};
use pm_render::wgpu;
use pm_render::{GpuContext, TARGET_FORMAT};

/// Warp-mesh vertex stage: computes the warped sampling UV (like the default
/// warp) and passes `uv`(=`vec4(warped, original)`) + `rad_ang` to the fragment.
const WARP_VERTEX: &str = r#"
struct WarpVtx {
    aspect: vec4<f32>,
    warp_factors: vec4<f32>,
    texel_offset: vec2<f32>,
    warp_time: f32,
    warp_scale_inverse: f32,
    decay: f32,
};
@group(0) @binding(0) var<uniform> warp: WarpVtx;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec4<f32>,
    @location(1) rad_ang: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) rad_ang: vec2<f32>,
    @location(2) transforms: vec4<f32>,
    @location(3) center: vec2<f32>,
    @location(4) distance: vec2<f32>,
    @location(5) stretch: vec2<f32>,
) -> VsOut {
    let pos = position;
    let radius = rad_ang.x;
    let zoom = transforms.x;
    let zoom_exp = transforms.y;
    let rot = transforms.z;
    let warp_amt = transforms.w;
    let aspect_x = warp.aspect.x;
    let aspect_y = warp.aspect.y;
    let inv_aspect_x = warp.aspect.z;
    let inv_aspect_y = warp.aspect.w;

    var out: VsOut;
    out.pos = vec4<f32>(pos.x, -pos.y, 0.0, 1.0);

    let zoom2 = pow(zoom, pow(zoom_exp, radius * 2.0 - 1.0));
    let zoom2_inv = 1.0 / zoom2;
    var uu = pos.x * aspect_x * 0.5 * zoom2_inv + 0.5;
    var vv = pos.y * aspect_y * 0.5 * zoom2_inv + 0.5;
    uu = (uu - center.x) / stretch.x + center.x;
    vv = (vv - center.y) / stretch.y + center.y;
    let wt = warp.warp_time;
    let wsi = warp.warp_scale_inverse;
    let wf = warp.warp_factors;
    uu = uu + warp_amt * 0.0035 * sin(wt * 0.333 + wsi * (pos.x * wf.x - pos.y * wf.w));
    vv = vv + warp_amt * 0.0035 * cos(wt * 0.375 - wsi * (pos.x * wf.z + pos.y * wf.y));
    uu = uu + warp_amt * 0.0035 * cos(wt * 0.753 - wsi * (pos.x * wf.y - pos.y * wf.z));
    vv = vv + warp_amt * 0.0035 * sin(wt * 0.825 + wsi * (pos.x * wf.x + pos.y * wf.w));
    let u2 = uu - center.x;
    let v2 = vv - center.y;
    let cr = cos(rot);
    let sr = sin(rot);
    uu = u2 * cr - v2 * sr + center.x;
    vv = u2 * sr + v2 * cr + center.y;
    uu = uu - distance.x;
    vv = vv - distance.y;
    uu = (uu - 0.5) * inv_aspect_x + 0.5;
    vv = (vv - 0.5) * inv_aspect_y + 0.5;
    uu = uu + warp.texel_offset.x;
    vv = vv + warp.texel_offset.y;

    let orig = vec2<f32>(pos.x * 0.5 + 0.5, pos.y * 0.5 + 0.5);
    out.uv = vec4<f32>(uu, vv, orig.x, orig.y);
    out.rad_ang = rad_ang;
    return out;
}
"#;

/// Assemble the full warp module: MdUniforms + warp-vertex uniforms + texture
/// bindings + the PS core + `load_uniforms` + the warp vertex + a fragment that
/// runs `PS`.
pub fn assemble(parts: &WarpShaderParts) -> String {
    let mut s = String::new();
    s.push_str(&md_uniforms_struct());
    s.push_str(WARP_VERTEX);
    s.push_str("@group(0) @binding(1) var<uniform> md: MdUniforms;\n");
    let mut binding = 2u32;
    for tex in &parts.textures {
        s.push_str(&format!("@group(0) @binding({binding}) var {tex}: texture_2d<f32>;\n"));
        binding += 1;
        s.push_str(&format!("@group(0) @binding({binding}) var {tex}_sampler: sampler;\n"));
        binding += 1;
    }
    s.push('\n');
    s.push_str(&parts.ps_wgsl);
    s.push('\n');
    s.push_str(&md_load_uniforms());
    s.push_str(
        "\n@fragment\nfn fs_main(in: VsOut) -> @location(0) vec4<f32> {\n    load_uniforms();\n    let o = PS(vec4<f32>(1.0, 1.0, 1.0, 1.0), in.uv, in.rad_ang);\n    return vec4<f32>(o._return_value.rgb, 1.0);\n}\n",
    );
    s
}

/// A compiled custom-warp pipeline plus its `MdUniforms` buffer.
pub struct CustomWarp {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub md_buf: wgpu::Buffer,
    pub texture_count: usize,
}

impl CustomWarp {
    /// Build the pipeline from a translated warp shader. Returns `None` if the
    /// generated WGSL fails to compile (so the renderer keeps the default warp).
    pub fn new(ctx: &GpuContext, parts: &WarpShaderParts) -> Option<Self> {
        let device = &ctx.device;
        let wgsl = assemble(parts);

        // Bind group layout: warp uniforms (vertex), md uniforms, textures.
        let mut entries = vec![
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
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ];
        for i in 0..parts.textures.len() {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: (2 + 2 * i) as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: (3 + 2 * i) as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("custom warp bgl"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("custom warp layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("custom warp shader"),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("custom warp pipeline"),
            layout: Some(&layout),
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
        if pollster::block_on(scope.pop()).is_some() {
            return None;
        }

        let md_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("custom warp md uniforms"),
            size: std::mem::size_of::<crate::md_uniforms::MdUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Some(CustomWarp { pipeline, bind_group_layout, md_buf, texture_count: parts.textures.len() })
    }
}
