//! Waveform/spectrum overlay renderer. A single fullscreen pass, alpha-blended
//! over the base visual (preset or shader), driven by projectM-derived stereo
//! audio (spectrum + waveform, both channels) — never `AnalyserNode`.
//!
//! Structured as a self-contained pass so Phase 6 can lift it into the layer
//! stack as an overlay layer source without a rewrite. Modes: 0 oscilloscope,
//! 1 mirrored, 2 spectrum bars, 3 circular waveform, 4 radial spectrum,
//! 5 Lissajous (uses real L/R). The audio texture is 512×4 R8Unorm:
//! row0 specL, row1 waveL, row2 specR, row3 waveR (centers 0.125/0.375/0.625/0.875).

use pm_audio::FrameAudioData;
use pm_render::{GpuContext, Texture, TARGET_FORMAT};

const TEX_W: u32 = 512;
const TEX_H: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct OverlayUniform {
    pub resolution: [f32; 2],
    pub mode: f32,
    pub channel: f32,
    pub color: [f32; 4], // rgb + opacity
    pub scale: f32,
    pub thickness: f32,
    pub rotation: f32,
    pub points: f32,
    pub position: [f32; 2],
    pub freq_min: f32,
    pub freq_max: f32,
    pub log_freq: f32,
    pub smoothing: f32,
    pub pad0: f32,
    pub pad1: f32,
}

impl Default for OverlayUniform {
    fn default() -> Self {
        OverlayUniform {
            resolution: [1.0, 1.0],
            mode: 0.0,
            channel: 0.0,
            color: [0.2, 0.95, 0.6, 0.9],
            scale: 0.35,
            thickness: 0.006,
            rotation: 0.0,
            points: 128.0,
            position: [0.0, 0.0],
            freq_min: 0.0,
            freq_max: 1.0,
            log_freq: 0.0,
            smoothing: 0.0,
            pad0: 0.0,
            pad1: 0.0,
        }
    }
}

impl From<OverlayUniform> for pm_scene::OverlayConfig {
    fn from(u: OverlayUniform) -> Self {
        pm_scene::OverlayConfig {
            mode: u.mode as u8,
            channel: u.channel as u8,
            color: u.color,
            scale: u.scale,
            thickness: u.thickness,
            rotation: u.rotation,
            points: u.points,
            log_freq: u.log_freq > 0.5,
        }
    }
}

impl From<pm_scene::OverlayConfig> for OverlayUniform {
    fn from(c: pm_scene::OverlayConfig) -> Self {
        OverlayUniform {
            mode: c.mode as f32,
            channel: c.channel as f32,
            color: c.color,
            scale: c.scale,
            thickness: c.thickness,
            rotation: c.rotation,
            points: c.points,
            log_freq: if c.log_freq { 1.0 } else { 0.0 },
            ..OverlayUniform::default()
        }
    }
}

const OVERLAY_WGSL: &str = r#"
struct OverlayU {
    resolution: vec2<f32>,
    mode: f32,
    channel: f32,
    color: vec4<f32>,
    scale: f32,
    thickness: f32,
    rotation: f32,
    points: f32,
    position: vec2<f32>,
    freq_min: f32,
    freq_max: f32,
    log_freq: f32,
    smoothing: f32,
    pad0: f32,
    pad1: f32,
};
@group(0) @binding(0) var<uniform> u: OverlayU;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;

const TAU = 6.2831853;

fn wav(x: f32, row: f32) -> f32 {
    return textureSampleLevel(tex, smp, vec2<f32>(x, row), 0.0).x - 0.5;
}
fn waveform_at(x: f32) -> f32 {
    if (u.channel < 0.5) { return wav(x, 0.375); }        // left
    else if (u.channel < 1.5) { return wav(x, 0.875); }   // right
    return 0.5 * (wav(x, 0.375) + wav(x, 0.875));          // mono
}
fn spectrum_at(x: f32) -> f32 {
    var xx = mix(u.freq_min, u.freq_max, clamp(x, 0.0, 1.0));
    if (u.log_freq > 0.5) { xx = xx * xx; }
    if (u.channel > 0.5 && u.channel < 1.5) {
        return textureSampleLevel(tex, smp, vec2<f32>(xx, 0.625), 0.0).x;
    }
    return textureSampleLevel(tex, smp, vec2<f32>(xx, 0.125), 0.0).x;
}

@vertex
fn vs(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[vid], 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    var uv = frag.xy / u.resolution;
    uv.y = 1.0 - uv.y; // to bottom-left origin
    let cc = uv - vec2<f32>(0.5) - u.position;
    let s = sin(u.rotation);
    let c = cos(u.rotation);
    let p = vec2<f32>(c * cc.x - s * cc.y, s * cc.x + c * cc.y);
    let uvp = p + vec2<f32>(0.5);

    let th = max(u.thickness, 0.001) + u.smoothing * 0.02;
    let mode = i32(u.mode + 0.5);
    var cover = 0.0;

    if (mode == 0) {
        let y = 0.5 + waveform_at(uvp.x) * u.scale;
        cover = smoothstep(th, 0.0, abs(uvp.y - y));
    } else if (mode == 1) {
        let a = abs(waveform_at(uvp.x)) * u.scale;
        cover = smoothstep(th, 0.0, abs(abs(uvp.y - 0.5) - a));
    } else if (mode == 2) {
        let h = spectrum_at(uvp.x) * u.scale * 2.0;
        cover = step(uvp.y, h) * step(0.0, uvp.y);
    } else if (mode == 3) {
        let d = uvp - vec2<f32>(0.5);
        let ang = atan2(d.y, d.x) / TAU + 0.5;
        let r = length(d);
        let rr = 0.3 + waveform_at(ang) * u.scale * 0.35;
        cover = smoothstep(th, 0.0, abs(r - rr));
    } else if (mode == 4) {
        let d = uvp - vec2<f32>(0.5);
        let ang = atan2(d.y, d.x) / TAU + 0.5;
        let r = length(d);
        let rr = 0.15 + spectrum_at(ang) * u.scale * 0.5;
        cover = step(r, rr) * step(0.12, r);
    } else {
        // Lissajous: min distance to the (waveL, waveR) parametric curve.
        let d = uvp - vec2<f32>(0.5);
        var mind = 1.0;
        let n = i32(clamp(u.points, 16.0, 256.0));
        for (var i = 0; i < n; i = i + 1) {
            let t = f32(i) / f32(n);
            let pt = vec2<f32>(wav(t, 0.375), wav(t, 0.875)) * u.scale;
            mind = min(mind, length(d - pt));
        }
        cover = smoothstep(th * 2.0, 0.0, mind);
    }

    return vec4<f32>(u.color.rgb, clamp(cover, 0.0, 1.0) * u.color.a);
}
"#;

pub struct OverlayRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    audio_tex: wgpu::Texture,
    upload: Vec<u8>,
    output: Texture,
    pub cfg: OverlayUniform,
}

impl OverlayRenderer {
    pub fn new(ctx: &GpuContext, width: u32, height: u32) -> Self {
        let device = &ctx.device;
        let format = TARGET_FORMAT;

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay bgl"),
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
            label: Some("overlay shader"),
            source: wgpu::ShaderSource::Wgsl(OVERLAY_WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Straight-alpha output into the layer texture; the
                    // compositor does the actual blending against the stack.
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

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay uniform"),
            size: std::mem::size_of::<OverlayUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let audio_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("overlay audio"),
            size: wgpu::Extent3d { width: TEX_W, height: TEX_H, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = audio_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });

        OverlayRenderer {
            pipeline,
            bind_group,
            uniform_buf,
            audio_tex,
            upload: vec![0u8; (TEX_W * TEX_H) as usize],
            output: Texture::new_render_target(device, "overlay-layer", width, height, format),
            cfg: OverlayUniform::default(),
        }
    }

    pub fn resize(&mut self, ctx: &GpuContext, width: u32, height: u32) {
        self.output = Texture::new_render_target(&ctx.device, "overlay-layer", width, height, TARGET_FORMAT);
    }

    pub fn output(&self) -> &Texture {
        &self.output
    }

    /// Fill the 512×4 stereo audio texture.
    pub fn update_audio(&mut self, ctx: &GpuContext, audio: &FrameAudioData) {
        let w = TEX_W as usize;
        let spec = |v: f32| ((v * 4.0).clamp(0.0, 1.0) * 255.0) as u8;
        let wave = |v: f32| ((0.5 + v / 256.0).clamp(0.0, 1.0) * 255.0) as u8;
        for x in 0..w {
            self.upload[x] = spec(audio.spectrum_left.get(x).copied().unwrap_or(0.0));
            self.upload[w + x] = wave(audio.waveform_left.get(x).copied().unwrap_or(0.0));
            self.upload[2 * w + x] = spec(audio.spectrum_right.get(x).copied().unwrap_or(0.0));
            self.upload[3 * w + x] = wave(audio.waveform_right.get(x).copied().unwrap_or(0.0));
        }
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.audio_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.upload,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(TEX_W),
                rows_per_image: Some(TEX_H),
            },
            wgpu::Extent3d { width: TEX_W, height: TEX_H, depth_or_array_layers: 1 },
        );
    }

    /// Render the overlay into its own texture (straight alpha, cleared first).
    pub fn render(&mut self, ctx: &GpuContext) {
        self.cfg.resolution = [self.output.width as f32, self.output.height as f32];
        ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&self.cfg));
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("overlay enc") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.output.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}
