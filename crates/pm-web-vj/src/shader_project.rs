//! Multipass Shadertoy shader project (Phase 8d): Buffer A–D + an Image pass,
//! executed on the existing WGSL path. This is the source renderer for a Shader
//! layer — it replaces the single-pass `LiveShader` while sharing the same
//! binding contract (uniform@0, iChannel0-3@1-4, sampler@5, user@6) so every
//! pass reuses one pipeline layout, audio texture, and user-control buffer.
//!
//! ## Execution semantics (documented; matches Shadertoy)
//! Passes execute in the fixed order **Buffer A → B → C → D → Image**. Each
//! buffer pass owns ping-pong history (`front` = most-recently-completed,
//! `back` = this frame's render target). A channel bound to a buffer reads that
//! buffer's `front`: because forward dependencies (an earlier buffer) have
//! already flipped this frame, they read **this frame's** output; a buffer
//! reading itself or a later buffer reads the **previous frame** — which
//! deterministically resolves every dependency cycle via one-frame history.
//! `iFrame` is the same for all passes in a frame and increments once per
//! rendered project frame. A pass that has never compiled outputs transparent;
//! a pass whose latest edit fails to compile keeps its last-known-good pipeline.

use pm_audio::FrameAudioData;
use pm_glsl::{compile_with, merge_controls, parse_controls, Control, Diagnostic, ShaderMode, ShaderUniforms, AUDIO_TEX_HEIGHT, AUDIO_TEX_WIDTH};
use pm_render::{GpuContext, Texture, TARGET_FORMAT};
use pm_scene::{pass_type_str, ChannelSource, PassData};

const USER_SLOTS: usize = 16;
const N_BUFFERS: usize = 4;
const IMAGE: usize = 4;
const N_PASSES: usize = 5;

const FULLSCREEN_VS: &str = r#"
@vertex
fn pm_vs(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[vid], 0.0, 1.0);
}
"#;

struct Pass {
    /// Configured (Image is always active; Buffer A–D become active when added).
    active: bool,
    /// User enable toggle (a disabled pass is skipped; its history freezes).
    enabled: bool,
    source: String,
    mode: u8,
    channels: [ChannelSource; 4],
    /// Last-known-good pipeline (retained across a failed recompile).
    pipeline: Option<wgpu::RenderPipeline>,
    /// Whether this pass has ever compiled successfully.
    compiled: bool,
    diagnostics: Vec<Diagnostic>,
    /// Ping-pong history: `front = history[which]`, `back = history[1-which]`.
    history: Option<[Texture; 2]>,
    which: usize,
    primed: bool,
}

impl Pass {
    fn new(source: String, mode: u8) -> Self {
        Pass {
            active: false,
            enabled: true,
            source,
            mode,
            channels: [ChannelSource::None; 4],
            pipeline: None,
            compiled: false,
            diagnostics: Vec::new(),
            history: None,
            which: 0,
            primed: false,
        }
    }
}

pub struct ShaderProject {
    format: wgpu::TextureFormat,
    bgl: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    uniform_buf: wgpu::Buffer,
    user_buf: wgpu::Buffer,
    audio_tex: wgpu::Texture,
    audio_view: wgpu::TextureView,
    placeholder_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    audio_upload: Vec<u8>,
    passes: [Pass; N_PASSES],
    controls: Vec<Control>,
    conflicts: Vec<String>,
    width: u32,
    height: u32,
}

impl ShaderProject {
    pub fn new(ctx: &GpuContext, width: u32, height: u32) -> Self {
        let device = &ctx.device;
        let format = TARGET_FORMAT;

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader-project bgl"),
            entries: &[
                buf_entry(0),
                tex_entry(1),
                tex_entry(2),
                tex_entry(3),
                tex_entry(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                buf_entry(6),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shader-project layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shader-project uniforms"),
            size: std::mem::size_of::<ShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let user_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shader-project user"),
            size: (USER_SLOTS * 16) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let audio_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shader-project audio"),
            size: wgpu::Extent3d { width: AUDIO_TEX_WIDTH, height: AUDIO_TEX_HEIGHT, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let audio_view = audio_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let placeholder = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shader-project placeholder"),
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
            label: Some("shader-project sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let mut passes: [Pass; N_PASSES] = std::array::from_fn(|_| Pass::new(String::new(), 0));
        // Image pass is always active; give it history so `output()` is valid.
        passes[IMAGE].active = true;
        passes[IMAGE].history = Some(new_history(device, format, width, height));

        let mut project = ShaderProject {
            format,
            bgl,
            pipeline_layout,
            uniform_buf,
            user_buf,
            audio_tex,
            audio_view,
            placeholder_view,
            sampler,
            audio_upload: vec![0u8; (AUDIO_TEX_WIDTH * AUDIO_TEX_HEIGHT) as usize],
            passes,
            controls: Vec::new(),
            conflicts: Vec::new(),
            width,
            height,
        };
        project.clear_all_history(ctx);
        project
    }

    // --- Introspection ------------------------------------------------------

    /// The Image pass's freshly-rendered output, sampled by the compositor.
    pub fn output(&self) -> &Texture {
        let p = &self.passes[IMAGE];
        &p.history.as_ref().expect("image always has history")[p.which]
    }

    pub fn controls(&self) -> &[Control] {
        &self.controls
    }
    /// Legacy single-pass fields for backward-compatible scene export.
    pub fn image_source(&self) -> (&str, u8) {
        (&self.passes[IMAGE].source, self.passes[IMAGE].mode)
    }
    pub fn active_pass_count(&self) -> usize {
        self.passes.iter().filter(|p| p.active && p.enabled).count()
    }
    pub fn buffer_pass_count(&self) -> usize {
        self.passes[..N_BUFFERS].iter().filter(|p| p.active && p.enabled).count()
    }

    // --- Editing ------------------------------------------------------------

    /// Add (activate) a Buffer pass A–D by index 0–3 with a default source.
    pub fn add_buffer(&mut self, ctx: &GpuContext, index: usize) -> bool {
        if index >= N_BUFFERS || self.passes[index].active {
            return false;
        }
        let letter = ["A", "B", "C", "D"][index];
        self.passes[index] = Pass::new(default_buffer_source(letter), 0);
        self.passes[index].active = true;
        self.passes[index].channels[0] = ChannelSource::SelfPrev;
        self.passes[index].history = Some(new_history(&ctx.device, self.format, self.width, self.height));
        clear_history(ctx, self.passes[index].history.as_ref().unwrap());
        self.passes[index].primed = true;
        self.recompile(ctx);
        true
    }

    /// Remove (deactivate) a Buffer pass and drop its history.
    pub fn remove_buffer(&mut self, ctx: &GpuContext, index: usize) {
        if index >= N_BUFFERS || !self.passes[index].active {
            return;
        }
        self.passes[index] = Pass::new(String::new(), 0);
        self.recompile(ctx);
    }

    /// Set a pass's source and recompile the project (returns that pass's
    /// diagnostics; other passes keep their last-known-good pipelines).
    pub fn set_pass_source(&mut self, ctx: &GpuContext, pass_index: usize, mode: ShaderMode, src: &str) -> Vec<Diagnostic> {
        if pass_index >= N_PASSES || !self.passes[pass_index].active {
            return vec![Diagnostic { line: 0, column: 0, message: "no such pass".into() }];
        }
        self.passes[pass_index].source = src.to_string();
        self.passes[pass_index].mode = if mode == ShaderMode::Raw { 1 } else { 0 };
        self.recompile(ctx);
        self.passes[pass_index].diagnostics.clone()
    }

    /// Configure `iChannelN` of a pass.
    pub fn set_channel(&mut self, pass_index: usize, channel: usize, source: ChannelSource) -> bool {
        if pass_index >= N_PASSES || channel >= 4 || !self.passes[pass_index].active {
            return false;
        }
        self.passes[pass_index].channels[channel] = source;
        true
    }

    pub fn set_pass_enabled(&mut self, pass_index: usize, enabled: bool) {
        if pass_index < N_PASSES && self.passes[pass_index].active {
            self.passes[pass_index].enabled = enabled;
        }
    }

    /// Clear all Buffer histories (feedback restart) without recompiling.
    pub fn reset_buffers(&mut self, ctx: &GpuContext) {
        self.clear_all_history(ctx);
    }

    fn clear_all_history(&mut self, ctx: &GpuContext) {
        for p in &mut self.passes {
            if let Some(h) = &p.history {
                clear_history(ctx, h);
            }
            p.which = 0;
            p.primed = p.history.is_some();
        }
    }

    pub fn resize(&mut self, ctx: &GpuContext, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        for p in &mut self.passes {
            if p.active {
                let h = new_history(&ctx.device, self.format, width, height);
                clear_history(ctx, &h);
                p.history = Some(h);
                p.which = 0;
                p.primed = true;
            }
        }
    }

    /// Re-parse controls across all active passes, then (re)compile each active
    /// pass with the merged project-level control registry. A pass that fails to
    /// compile keeps its previous pipeline (last-known-good).
    fn recompile(&mut self, ctx: &GpuContext) {
        let per_pass: Vec<Vec<Control>> = self
            .passes
            .iter()
            .map(|p| if p.active { parse_controls(&p.source) } else { Vec::new() })
            .collect();
        let (merged, conflicts) = merge_controls(&per_pass);
        self.controls = merged;
        self.conflicts = conflicts;

        for i in 0..N_PASSES {
            if !self.passes[i].active {
                self.passes[i].pipeline = None;
                self.passes[i].compiled = false;
                continue;
            }
            let mode = if self.passes[i].mode == 1 { ShaderMode::Raw } else { ShaderMode::Shadertoy };
            let src = self.passes[i].source.clone();
            match compile_with(mode, &src, &self.controls) {
                Ok(t) => {
                    let module = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("shader-project module"),
                        source: wgpu::ShaderSource::Wgsl(format!("{}\n{FULLSCREEN_VS}", t.wgsl).into()),
                    });
                    let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("shader-project pipeline"),
                        layout: Some(&self.pipeline_layout),
                        vertex: wgpu::VertexState { module: &module, entry_point: Some("pm_vs"), buffers: &[], compilation_options: Default::default() },
                        fragment: Some(wgpu::FragmentState {
                            module: &module,
                            entry_point: Some("main"),
                            targets: &[Some(wgpu::ColorTargetState { format: self.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                            compilation_options: Default::default(),
                        }),
                        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        multiview_mask: None,
                        cache: None,
                    });
                    self.passes[i].pipeline = Some(pipeline);
                    self.passes[i].compiled = true;
                    self.passes[i].diagnostics.clear();
                }
                Err(diags) => {
                    // Keep last-known-good pipeline; only record diagnostics.
                    self.passes[i].diagnostics = diags;
                }
            }
        }
    }

    // --- Scene serialization ------------------------------------------------

    /// Serialize the multipass project. Only active passes are emitted; the
    /// Image pass is always first-class.
    pub fn to_passes(&self) -> Vec<PassData> {
        let mut out = Vec::new();
        for i in 0..N_PASSES {
            let p = &self.passes[i];
            if !p.active {
                continue;
            }
            out.push(PassData {
                pass_type: pass_type_str(i).to_string(),
                enabled: p.enabled,
                source: p.source.clone(),
                mode: p.mode,
                channels: std::array::from_fn(|c| p.channels[c].as_str().to_string()),
            });
        }
        out
    }

    /// Rebuild the project from serialized passes (histories start clean).
    pub fn load_passes(&mut self, ctx: &GpuContext, passes: &[PassData]) {
        // Reset every pass to inactive, then activate from data.
        for i in 0..N_PASSES {
            self.passes[i] = Pass::new(String::new(), 0);
        }
        for pd in passes {
            let Some(i) = pd.index() else { continue };
            self.passes[i].active = true;
            self.passes[i].enabled = pd.enabled;
            self.passes[i].source = pd.source.clone();
            self.passes[i].mode = pd.mode;
            for c in 0..4 {
                self.passes[i].channels[c] = ChannelSource::parse(&pd.channels[c]);
            }
        }
        // Image must always exist.
        if !self.passes[IMAGE].active {
            self.passes[IMAGE].active = true;
        }
        for i in 0..N_PASSES {
            if self.passes[i].active {
                let h = new_history(&ctx.device, self.format, self.width, self.height);
                clear_history(ctx, &h);
                self.passes[i].history = Some(h);
                self.passes[i].which = 0;
                self.passes[i].primed = true;
            }
        }
        self.recompile(ctx);
    }

    /// Compact UI description of the project.
    pub fn project_json(&self) -> String {
        let passes: Vec<String> = (0..N_PASSES)
            .filter(|&i| self.passes[i].active)
            .map(|i| {
                let p = &self.passes[i];
                let chans: Vec<String> = p.channels.iter().map(|c| format!("\"{}\"", c.as_str())).collect();
                let diags: Vec<String> = p
                    .diagnostics
                    .iter()
                    .map(|d| format!("{{\"line\":{},\"column\":{},\"message\":{}}}", d.line, d.column, json_str(&d.message)))
                    .collect();
                format!(
                    "{{\"type\":\"{}\",\"index\":{},\"enabled\":{},\"mode\":{},\"compiled\":{},\"source\":{},\"channels\":[{}],\"diagnostics\":[{}]}}",
                    pass_type_str(i),
                    i,
                    p.enabled,
                    p.mode,
                    p.compiled,
                    json_str(&p.source),
                    chans.join(","),
                    diags.join(",")
                )
            })
            .collect();
        let conflicts: Vec<String> = self.conflicts.iter().map(|c| json_str(c)).collect();
        format!("{{\"passes\":[{}],\"conflicts\":[{}]}}", passes.join(","), conflicts.join(","))
    }

    // --- Rendering ----------------------------------------------------------

    pub fn update_user_controls(&self, ctx: &GpuContext, slots: &[[f32; 4]; USER_SLOTS]) {
        ctx.queue.write_buffer(&self.user_buf, 0, bytemuck::cast_slice(slots));
    }

    fn upload_audio(&mut self, ctx: &GpuContext, audio: &FrameAudioData) {
        let w = AUDIO_TEX_WIDTH as usize;
        for x in 0..w {
            let v = audio.spectrum_left.get(x).copied().unwrap_or(0.0);
            self.audio_upload[x] = ((v * 4.0).clamp(0.0, 1.0) * 255.0) as u8;
        }
        for x in 0..w {
            let v = audio.waveform_left.get(x).copied().unwrap_or(0.0);
            self.audio_upload[w + x] = ((0.5 + v / 256.0).clamp(0.0, 1.0) * 255.0) as u8;
        }
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.audio_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &self.audio_upload,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(AUDIO_TEX_WIDTH), rows_per_image: Some(AUDIO_TEX_HEIGHT) },
            wgpu::Extent3d { width: AUDIO_TEX_WIDTH, height: AUDIO_TEX_HEIGHT, depth_or_array_layers: 1 },
        );
    }

    /// The `front` (most-recently-completed) view of a buffer/self channel.
    fn front_view(&self, pass: usize) -> &wgpu::TextureView {
        let p = &self.passes[pass];
        &p.history.as_ref().unwrap()[p.which].view
    }

    fn channel_view(&self, reader: usize, src: ChannelSource) -> &wgpu::TextureView {
        match src {
            ChannelSource::None => &self.placeholder_view,
            ChannelSource::Audio => &self.audio_view,
            ChannelSource::SelfPrev => self.front_view(reader),
            ChannelSource::Buffer(b) => {
                let b = b as usize;
                if b < N_BUFFERS && self.passes[b].active {
                    self.front_view(b)
                } else {
                    &self.placeholder_view
                }
            }
        }
    }

    /// Execute the whole project for this frame. `base` carries the frame's
    /// global timing/audio uniforms (iTime/iFrame/iBass/… — same for every pass).
    pub fn render(&mut self, ctx: &GpuContext, audio: &FrameAudioData, base: &ShaderUniforms) {
        self.upload_audio(ctx, audio);
        let (w, h) = (self.width, self.height);

        for i in 0..N_PASSES {
            if !self.passes[i].active || !self.passes[i].enabled {
                continue;
            }
            let which = self.passes[i].which;
            let back = 1 - which;

            // Per-pass uniforms: iChannelResolution reflects each bound source.
            let mut u = *base;
            u.i_resolution = [w as f32, h as f32, 1.0];
            for c in 0..4 {
                u.i_channel_resolution[c] = match self.passes[i].channels[c] {
                    ChannelSource::Audio => [AUDIO_TEX_WIDTH as f32, AUDIO_TEX_HEIGHT as f32, 1.0, 0.0],
                    ChannelSource::Buffer(b) if (b as usize) < N_BUFFERS && self.passes[b as usize].active => [w as f32, h as f32, 1.0, 0.0],
                    ChannelSource::SelfPrev => [w as f32, h as f32, 1.0, 0.0],
                    _ => [1.0, 1.0, 1.0, 0.0],
                };
            }
            ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));

            // Resolve channel views + build the per-pass bind group.
            let views: [&wgpu::TextureView; 4] = std::array::from_fn(|c| self.channel_view(i, self.passes[i].channels[c]));
            let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shader-project bg"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(views[0]) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(views[1]) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(views[2]) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(views[3]) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 6, resource: self.user_buf.as_entire_binding() },
                ],
            });

            let target = &self.passes[i].history.as_ref().unwrap()[back].view;
            let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shader-project enc") });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("shader-project pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                if let Some(pipeline) = &self.passes[i].pipeline {
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &bind, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
            ctx.queue.submit(Some(encoder.finish()));

            // Flip so `front` now points at this frame's output.
            self.passes[i].which = back;
        }
    }
}

fn new_history(device: &wgpu::Device, format: wgpu::TextureFormat, w: u32, h: u32) -> [Texture; 2] {
    [
        Texture::new_render_target(device, "shader-pass-0", w, h, format),
        Texture::new_render_target(device, "shader-pass-1", w, h, format),
    ]
}

fn clear_history(ctx: &GpuContext, h: &[Texture; 2]) {
    for t in h {
        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shader-pass clear") });
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shader-pass clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &t.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        ctx.queue.submit(Some(encoder.finish()));
    }
}

fn default_buffer_source(letter: &str) -> String {
    format!(
        "// Buffer {letter}: previous-frame feedback (iChannel0 = Self)\n\
         void mainImage(out vec4 fragColor, in vec2 fragCoord) {{\n\
         \tvec2 uv = fragCoord / iResolution.xy;\n\
         \tvec4 prev = texture(iChannel0, uv);\n\
         \tfragColor = prev * 0.97 + 0.03 * vec4(uv, 0.5 + 0.5 * sin(iTime), 1.0);\n\
         }}\n"
    )
}

fn buf_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}
fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
