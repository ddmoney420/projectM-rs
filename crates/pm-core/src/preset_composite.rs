//! Renders a preset's **custom composite shader** (translated to WGSL by
//! `pm_preset::preset_shader`) instead of the built-in hue composite.
//!
//! Binds the warp feedback buffer as `sampler_main` (and as a stand-in for the
//! blur / user textures, which aren't built yet), fills the [`MdUniforms`]
//! block from preset state each frame, and draws fullscreen to a display target.

use crate::md_uniforms::MdUniforms;
use crate::noise::NoiseTextures;
use pm_preset::{is_3d_sampler, PresetState, TranslatedShader};
use pm_render::wgpu;
use pm_render::{GpuContext, Texture, TARGET_FORMAT};

pub struct PresetComposite {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    sampler: wgpu::Sampler,
    texture_count: usize,
    /// Sampler names in binding order, to bind noise textures vs. the feedback.
    texture_names: Vec<String>,
    output: Texture,
}

impl PresetComposite {
    /// Build a pipeline from a translated composite shader. Returns `None` if
    /// the WGSL fails to compile (so the caller falls back to the default).
    pub fn new(ctx: &GpuContext, shader: &TranslatedShader, width: u32, height: u32) -> Option<Self> {
        let device = &ctx.device;

        // Bind group layout: uniform at 0, then (texture, sampler) pairs.
        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }];
        for (i, tex) in shader.textures.iter().enumerate() {
            let view_dimension = if is_3d_sampler(tex) {
                wgpu::TextureViewDimension::D3
            } else {
                wgpu::TextureViewDimension::D2
            };
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: (2 * i + 1) as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension,
                    multisampled: false,
                },
                count: None,
            });
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: (2 * i + 2) as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preset composite bgl"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preset composite layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // Catch shader/pipeline validation errors instead of panicking.
        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preset composite shader"),
            source: wgpu::ShaderSource::Wgsl(shader.wgsl.clone().into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preset composite pipeline"),
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
        // Native: block to check shader validation and fall back on failure.
        // Single-threaded wasm can't block_on (panics "condvar wait not
        // supported"), so pop the scope without awaiting; a validation failure
        // surfaces as an uncaptured wgpu error rather than a clean fallback.
        #[cfg(not(target_arch = "wasm32"))]
        if pollster::block_on(error_scope.pop()).is_some() {
            return None; // shader rejected by wgpu
        }
        #[cfg(target_arch = "wasm32")]
        let _ = error_scope.pop();

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("md uniforms"),
            size: std::mem::size_of::<MdUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("preset composite sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let output = Texture::new_render_target(device, "preset composite output", width, height, TARGET_FORMAT);

        Some(PresetComposite {
            pipeline,
            bind_group_layout,
            uniform_buf,
            sampler,
            texture_count: shader.textures.len(),
            texture_names: shader.textures.clone(),
            output,
        })
    }

    pub fn output(&self) -> &Texture {
        &self.output
    }

    /// Render the composite, sampling `main` (the warp feedback buffer) and the
    /// shared `noise` textures for any `sampler_noise*` references.
    pub fn draw(
        &self,
        ctx: &GpuContext,
        main: &Texture,
        state: &PresetState,
        time: f32,
        noise: &NoiseTextures,
        blur: &crate::blur::Blur,
    ) {
        let uniforms = MdUniforms::from_state(state, time);
        ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: self.uniform_buf.as_entire_binding(),
        }];
        for i in 0..self.texture_count {
            // Noise/blur samplers resolve to the matching texture; everything
            // else (main, user textures) to the feedback buffer.
            let view = match self.texture_names.get(i).and_then(|n| noise.get(n).or_else(|| blur.get(n))) {
                Some(tex) => &tex.view,
                None => &main.view,
            };
            entries.push(wgpu::BindGroupEntry {
                binding: (2 * i + 1) as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: (2 * i + 2) as u32,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            });
        }
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preset composite bg"),
            layout: &self.bind_group_layout,
            entries: &entries,
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("preset composite encoder") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preset composite pass"),
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
