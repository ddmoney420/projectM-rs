//! A tiny in-window HUD overlay: a few lines of text drawn in the top-left
//! corner over the final image, with no new dependencies.
//!
//! Text is rasterized on the CPU with an embedded 5×7 bitmap font into a small
//! RGBA texture (only when the text changes), then composited over the surface
//! by an alpha-blended fullscreen pass that runs *after* the blit — so the
//! visualizer's framebuffer and feedback buffers are never touched. The font is
//! uppercase-only; lowercase input is upcased and unsupported glyphs render as
//! blanks (see `glyph`).

use pm_render::wgpu;
use pm_render::GpuContext;

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
const ADVANCE: usize = GLYPH_W + 1; // 1px inter-glyph gap
const LINE_H: usize = GLYPH_H + 1; // 1px inter-line gap
const PAD: usize = 2; // dark-box padding around the text
const SCALE: f32 = 2.0; // integer upscale for legibility
const MARGIN: f32 = 8.0; // px from the top-left corner

/// 5×7 glyph rows (top→bottom); each row's low 5 bits are columns, bit4 = left.
/// Returns all-zero (blank) for unsupported characters.
fn glyph(c: char) -> [u8; GLYPH_H] {
    let c = c.to_ascii_uppercase();
    match c {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x06],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x06, 0x04, 0x08],
        '-' => [0x00, 0x00, 0x00, 0x0E, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],
        '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        ':' => [0x00, 0x06, 0x06, 0x00, 0x06, 0x06, 0x00],
        '%' => [0x19, 0x19, 0x02, 0x04, 0x08, 0x13, 0x13],
        '[' => [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E],
        ']' => [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        '\'' => [0x04, 0x04, 0x04, 0x00, 0x00, 0x00, 0x00],
        '·' => [0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00],
        _ => [0; GLYPH_H], // space + unsupported -> blank
    }
}

/// Rasterize `lines` into an RGBA8 buffer (white text on a translucent dark
/// box). Returns `(pixels, width, height)`. Pure/CPU — unit-testable.
pub fn rasterize(lines: &[String]) -> (Vec<u8>, u32, u32) {
    let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let text_w = cols * ADVANCE;
    let text_h = lines.len() * LINE_H;
    let w = (text_w + PAD * 2).max(1);
    let h = (text_h + PAD * 2).max(1);
    // Background: translucent dark box.
    let mut px = vec![0u8; w * h * 4];
    for p in px.chunks_exact_mut(4) {
        p[0] = 0;
        p[1] = 0;
        p[2] = 0;
        p[3] = 150;
    }
    let set = |px: &mut [u8], x: usize, y: usize| {
        let i = (y * w + x) * 4;
        px[i] = 235;
        px[i + 1] = 235;
        px[i + 2] = 235;
        px[i + 3] = 255;
    };
    for (row, line) in lines.iter().enumerate() {
        let oy = PAD + row * LINE_H;
        for (col, ch) in line.chars().enumerate() {
            let ox = PAD + col * ADVANCE;
            let g = glyph(ch);
            for (gy, bits) in g.iter().enumerate() {
                for gx in 0..GLYPH_W {
                    if bits & (1 << (GLYPH_W - 1 - gx)) != 0 {
                        set(&mut px, ox + gx, oy + gy);
                    }
                }
            }
        }
    }
    (px, w as u32, h as u32)
}

/// GPU overlay that composites the rasterized HUD text over the surface.
pub struct Hud {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    texture: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
    last_text: Vec<String>,
}

const HUD_WGSL: &str = r#"
struct HudU {
    surf_tex: vec4<f32>, // surf.xy, tex.xy (pixels)
    params:   vec4<f32>, // scale, margin_x, margin_y, _pad
};
@group(0) @binding(0) var<uniform> u: HudU;
@group(0) @binding(1) var hud_tex: texture_2d<f32>;
@group(0) @binding(2) var hud_smp: sampler;

struct VsOut { @builtin(position) pos: vec4<f32> };

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(corners[vid], 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = u.surf_tex.zw;
    let local = (in.pos.xy - vec2<f32>(u.params.y, u.params.z)) / u.params.x;
    if (local.x < 0.0 || local.y < 0.0 || local.x >= tex.x || local.y >= tex.y) {
        return vec4<f32>(0.0); // outside the HUD box: leave the image untouched
    }
    return textureSample(hud_tex, hud_smp, local / tex);
}
"#;

impl Hud {
    pub fn new(ctx: &GpuContext, surface_format: wgpu::TextureFormat) -> Self {
        let device = &ctx.device;
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hud bgl"),
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
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hud shader"),
            source: wgpu::ShaderSource::Wgsl(HUD_WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hud layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud pipeline"),
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
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
            label: Some("hud sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud uniform"),
            size: 32, // two vec4<f32>
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Hud { pipeline, bgl, sampler, uniform, texture: None, last_text: Vec::new() }
    }

    /// Re-rasterize and re-upload the HUD texture only when `lines` changed.
    pub fn update(&mut self, ctx: &GpuContext, lines: &[String]) {
        if self.last_text == lines && self.texture.is_some() {
            return;
        }
        let (px, w, h) = rasterize(lines);
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hud texture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &px,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.texture = Some((texture, view, w, h));
        self.last_text = lines.to_vec();
    }

    /// Composite the HUD over `target` (the surface view). No-op if no texture.
    pub fn draw(&self, ctx: &GpuContext, target: &wgpu::TextureView, surf_w: u32, surf_h: u32) {
        let Some((_, view, tw, th)) = &self.texture else { return };
        let uni: [f32; 8] = [
            surf_w as f32,
            surf_h as f32,
            *tw as f32,
            *th as f32,
            SCALE,
            MARGIN,
            MARGIN,
            0.0,
        ];
        ctx.queue.write_buffer(&self.uniform, 0, bytemuck_cast(&uni));

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hud bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });

        let mut encoder =
            ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("hud encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hud pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    // Load (don't clear) so the HUD composites over the blit.
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
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

/// Reinterpret an `[f32; 8]` as bytes for an upload (no external dependency).
fn bytemuck_cast(v: &[f32; 8]) -> &[u8] {
    // Safety: `[f32; 8]` is 32 contiguous bytes with no padding/invalid values.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of::<[f32; 8]>()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterize_sizes_and_draws_glyphs() {
        let lines = vec!["AB 0".to_string()];
        let (px, w, h) = rasterize(&lines);
        // 4 glyphs * 6px advance + 2*2 pad = 28 wide; 1 line * 8 + 4 = 12 tall.
        assert_eq!(w, 28);
        assert_eq!(h, 12);
        assert_eq!(px.len() as u32, w * h * 4);
        // Background is translucent black; at least some pixels are lit white.
        let lit = px.chunks_exact(4).filter(|p| p[0] > 200).count();
        assert!(lit > 10, "glyphs drew lit pixels: {lit}");
    }

    #[test]
    fn empty_lines_safe() {
        let (px, w, h) = rasterize(&[]);
        assert!(w >= 1 && h >= 1 && !px.is_empty());
    }
}
