//! Live GLSL shader pipeline for the console. Translates user GLSL → WGSL via
//! `pm_glsl`, builds a fullscreen wgpu pipeline with the fixed binding layout
//! naga emits (uniform @0, channel textures @1..4, sampler @5), and renders it.
//!
//! Compilation is synchronous from JS's perspective (naga validates before any
//! GPU object is made), so there is no async race: a newer compile always wins,
//! and a failed compile returns diagnostics while the last-known-good pipeline
//! keeps rendering.

use pm_audio::FrameAudioData;
use pm_glsl::{compile, Control, Diagnostic, ShaderMode, ShaderUniforms, AUDIO_TEX_HEIGHT, AUDIO_TEX_WIDTH};
use pm_render::GpuContext;

/// Number of `vec4` user-control slots (mirrors `pm_glsl::MAX_CONTROLS`).
const USER_SLOTS: usize = 16;

/// Fullscreen-triangle vertex stage, appended to every translated fragment
/// module. The fragment entry naga emits is `main(@builtin(position) ..)`, so
/// the vertex stage only needs to output clip position.
const FULLSCREEN_VS: &str = r#"
@vertex
fn pm_vs(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(p[vid], 0.0, 1.0);
}
"#;

pub struct CompileOutcome {
    pub ok: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub controls: Vec<Control>,
}

pub struct LiveShader {
    format: wgpu::TextureFormat,
    pipeline_layout: wgpu::PipelineLayout,
    uniform_buf: wgpu::Buffer,
    user_buf: wgpu::Buffer,
    audio_tex: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    pipeline: Option<wgpu::RenderPipeline>,
    audio_upload: Vec<u8>,
}

impl LiveShader {
    pub fn new(ctx: &GpuContext, format: wgpu::TextureFormat) -> Self {
        let device = &ctx.device;

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("live-shader bgl"),
            entries: &[
                // 0: uniforms
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
                // 1..4: channel textures
                tex_entry(1),
                tex_entry(2),
                tex_entry(3),
                tex_entry(4),
                // 5: shared sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // 6: user controls (pm_user[16])
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("live-shader layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("live-shader uniforms"),
            size: std::mem::size_of::<ShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let user_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("live-shader user controls"),
            size: (USER_SLOTS * 16) as u64, // 16 vec4
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let audio_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("live-shader audio"),
            size: wgpu::Extent3d {
                width: AUDIO_TEX_WIDTH,
                height: AUDIO_TEX_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let audio_view = audio_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // 1×1 black placeholder for iChannel1..3 (asset loading not implemented).
        let placeholder = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("live-shader placeholder"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let placeholder_view = placeholder.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("live-shader sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("live-shader bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&audio_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&placeholder_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&placeholder_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&placeholder_view) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 6, resource: user_buf.as_entire_binding() },
            ],
        });

        LiveShader {
            format,
            pipeline_layout,
            uniform_buf,
            user_buf,
            audio_tex,
            bind_group,
            pipeline: None,
            audio_upload: vec![0u8; (AUDIO_TEX_WIDTH * AUDIO_TEX_HEIGHT) as usize],
        }
    }

    /// Upload the 16 user-control `vec4` slots.
    pub fn update_user_controls(&self, ctx: &GpuContext, slots: &[[f32; 4]; USER_SLOTS]) {
        ctx.queue.write_buffer(&self.user_buf, 0, bytemuck::cast_slice(slots));
    }

    pub fn has_pipeline(&self) -> bool {
        self.pipeline.is_some()
    }

    /// Compile user GLSL and, on success, atomically swap in the new pipeline.
    /// On failure the previous pipeline is kept and diagnostics are returned.
    pub fn set_shader(&mut self, ctx: &GpuContext, mode: ShaderMode, src: &str) -> CompileOutcome {
        match compile(mode, src) {
            Ok(translated) => {
                let wgsl = format!("{}\n{FULLSCREEN_VS}", translated.wgsl);
                let module = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("live-shader module"),
                    source: wgpu::ShaderSource::Wgsl(wgsl.into()),
                });
                let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("live-shader pipeline"),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("pm_vs"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: self.format,
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
                self.pipeline = Some(pipeline);
                CompileOutcome { ok: true, diagnostics: Vec::new(), controls: translated.controls }
            }
            Err(diagnostics) => CompileOutcome { ok: false, diagnostics, controls: Vec::new() },
        }
    }

    pub fn update_uniforms(&self, ctx: &GpuContext, u: &ShaderUniforms) {
        ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(u));
    }

    /// Upload spectrum (row 0) + waveform (row 1) into the audio texture.
    pub fn update_audio(&mut self, ctx: &GpuContext, audio: &FrameAudioData) {
        let w = AUDIO_TEX_WIDTH as usize;
        // Row 0: spectrum, modest gain then clamp to [0,255].
        for x in 0..w {
            let v = audio.spectrum_left.get(x).copied().unwrap_or(0.0);
            self.audio_upload[x] = ((v * 4.0).clamp(0.0, 1.0) * 255.0) as u8;
        }
        // Row 1: waveform mapped 0.5 + x/256 → [0,1].
        for x in 0..w {
            let v = audio.waveform_left.get(x).copied().unwrap_or(0.0);
            self.audio_upload[w + x] = ((0.5 + v / 256.0).clamp(0.0, 1.0) * 255.0) as u8;
        }
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.audio_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.audio_upload,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(AUDIO_TEX_WIDTH),
                rows_per_image: Some(AUDIO_TEX_HEIGHT),
            },
            wgpu::Extent3d {
                width: AUDIO_TEX_WIDTH,
                height: AUDIO_TEX_HEIGHT,
                depth_or_array_layers: 1,
            },
        );
    }

    pub fn render(&self, ctx: &GpuContext, target: &wgpu::TextureView) {
        let Some(pipeline) = &self.pipeline else { return };
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("live-shader enc") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("live-shader pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
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
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
