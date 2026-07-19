//! The layer compositor: an ordered stack of layers blended into an opaque
//! accumulator via bounded ping-pong. Replaces the Phase 5 "Milkdrop OR shader
//! + special overlay pass" with a real render graph:
//!
//! ```text
//! source render → layer texture → composite onto accumulator → next layer → blit
//! ```
//!
//! Three full-res intermediates only (two ping-pong accumulators + each layer's
//! own output texture) — never a texture-per-frame chain. Blend math mirrors
//! `pm_scene::blend` (unit-tested there). One Milkdrop engine instance is shared
//! (single Milkdrop layer, enforced); shader/waveform/spectrum layers each own
//! their renderer and last-known-good state.

use pm_audio::FrameAudioData;
use pm_glsl::{Control, ShaderMode, ShaderUniforms};
use pm_params::{ModContext, Parameter};
use pm_render::{Blit, GpuContext, Texture, TARGET_FORMAT};
use pm_scene::{
    Attribution, BlendMode, LayerData, ModMapping, OverlayConfig, SceneData, SourceState, Transform,
    SCHEMA_VERSION,
};

use crate::effects::{EffectChain, EffectKind, Effects};
use crate::live_shader::{CompileOutcome, LiveShader};
use crate::overlay::OverlayRenderer;

const USER_SLOTS: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    Milkdrop,
    Shader,
    Waveform,
    Spectrum,
}

impl LayerKind {
    fn default_name(self) -> &'static str {
        match self {
            LayerKind::Milkdrop => "Milkdrop",
            LayerKind::Shader => "Shader",
            LayerKind::Waveform => "Waveform",
            LayerKind::Spectrum => "Spectrum",
        }
    }
}

/// A shader layer's editable state (source + user controls + attribution).
struct ShaderState {
    shader: LiveShader,
    source: String,
    mode: u8,
    user_slots: [[f32; 4]; USER_SLOTS],
    user_mods: [Option<Parameter>; USER_SLOTS],
    user_range: [[f32; 2]; USER_SLOTS],
    controls: Vec<Control>,
    attribution: Attribution,
}

enum Runtime {
    Milkdrop,
    Shader(ShaderState),
    Waveform(OverlayRenderer),
    Spectrum(OverlayRenderer),
}

struct Layer {
    id: u64,
    name: String,
    enabled: bool,
    visible: bool,
    opacity: f32,
    blend: BlendMode,
    transform: Transform,
    runtime: Runtime,
    effects: EffectChain,
    /// Where this layer's effect chain writes (sampled by the compositor when
    /// the layer has enabled effects).
    effect_output: Texture,
}

impl Layer {
    fn kind(&self) -> LayerKind {
        match self.runtime {
            Runtime::Milkdrop => LayerKind::Milkdrop,
            Runtime::Shader(_) => LayerKind::Shader,
            Runtime::Waveform(_) => LayerKind::Waveform,
            Runtime::Spectrum(_) => LayerKind::Spectrum,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CompUniform {
    m: [f32; 4],
    pos: [f32; 2],
    resolution: [f32; 2],
    opacity: f32,
    blend: f32,
    opaque: f32,
    _pad: f32,
}

const COMPOSITE_WGSL: &str = r#"
struct U {
    m: vec4<f32>,
    pos: vec2<f32>,
    resolution: vec2<f32>,
    opacity: f32,
    blend: f32,
    opaque: f32,
    pad: f32,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var accum: texture_2d<f32>;
@group(0) @binding(2) var layer: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;

@vertex
fn vs(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[vid], 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag.xy / u.resolution;
    let dst = textureSample(accum, smp, uv).rgb;

    // Inverse transform: sample the layer at its placed position. The sample is
    // UNCONDITIONAL (uniform control flow — WGSL forbids implicit-derivative
    // sampling inside per-pixel branches); we mask out-of-bounds afterward.
    let d = uv - vec2<f32>(0.5) - u.pos;
    let luv = vec2<f32>(0.5) + vec2<f32>(u.m.x * d.x + u.m.y * d.y, u.m.z * d.x + u.m.w * d.y);
    let s = textureSample(layer, smp, luv);
    let inside = select(0.0, 1.0, all(luv >= vec2<f32>(0.0)) && all(luv <= vec2<f32>(1.0)));

    let srcRGB = s.rgb;
    let srcA = s.a;
    // Opaque base layers (Milkdrop/shader) ignore their source alpha; overlays
    // use their coverage alpha. `inside` masks the transformed region.
    let base_a = select(srcA, 1.0, u.opaque > 0.5) * inside;
    let a = clamp(base_a * u.opacity, 0.0, 1.0);
    let mode = i32(u.blend + 0.5);

    if (mode == 1) { // add
        return vec4<f32>(clamp(dst + srcRGB * a, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    }
    var b = srcRGB;                                    // normal
    if (mode == 2) { b = 1.0 - (1.0 - dst) * (1.0 - srcRGB); }      // screen
    else if (mode == 3) { b = dst * srcRGB; }                       // multiply
    else if (mode == 4) { b = abs(dst - srcRGB); }                  // difference
    else if (mode == 5) { b = max(dst, srcRGB); }                   // lighten
    else if (mode == 6) { b = min(dst, srcRGB); }                   // darken
    return vec4<f32>(mix(dst, b, a), 1.0);
}
"#;

pub struct Compositor {
    layers: Vec<Layer>,
    next_id: u64,
    next_effect_id: u64,
    selected: Option<u64>,
    accum_a: Texture,
    accum_b: Texture,
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    blit: Blit,
    effects: Effects,
    global_chain: EffectChain,
    global_output: Texture,
    width: u32,
    height: u32,
}

impl Compositor {
    pub fn new(ctx: &GpuContext, surface_format: wgpu::TextureFormat, width: u32, height: u32) -> Self {
        let device = &ctx.device;
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite bgl"),
            entries: &[
                buf_entry(0),
                tex_entry(1),
                tex_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite pipeline"),
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
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("composite uniform"),
            size: std::mem::size_of::<CompUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut c = Compositor {
            layers: Vec::new(),
            next_id: 1,
            next_effect_id: 1,
            selected: None,
            accum_a: Texture::new_render_target(device, "accum-a", width, height, TARGET_FORMAT),
            accum_b: Texture::new_render_target(device, "accum-b", width, height, TARGET_FORMAT),
            pipeline,
            bgl,
            sampler,
            uniform_buf,
            blit: Blit::new(ctx, surface_format),
            effects: Effects::new(ctx, width, height),
            global_chain: EffectChain::default(),
            global_output: Texture::new_render_target(device, "global-fx", width, height, TARGET_FORMAT),
            width,
            height,
        };
        c.load_default(ctx);
        c
    }

    /// The default scene: a Milkdrop layer under a waveform overlay.
    pub fn load_default(&mut self, ctx: &GpuContext) {
        self.layers.clear();
        self.next_id = 1;
        self.global_chain = EffectChain::default();
        let m = self.add_layer(ctx, LayerKind::Milkdrop).unwrap();
        let w = self.add_layer(ctx, LayerKind::Waveform).unwrap();
        self.selected = Some(m);
        let _ = w;
    }

    // --- Effect chains (target 0 = global, else the layer id) --------------

    fn chain_mut(&mut self, target: u64) -> Option<&mut EffectChain> {
        if target == 0 {
            Some(&mut self.global_chain)
        } else {
            self.layers.iter_mut().find(|l| l.id == target).map(|l| &mut l.effects)
        }
    }
    fn chain(&self, target: u64) -> Option<&EffectChain> {
        if target == 0 {
            Some(&self.global_chain)
        } else {
            self.layers.iter().find(|l| l.id == target).map(|l| &l.effects)
        }
    }

    pub fn add_effect(&mut self, target: u64, type_str: &str) -> Option<u64> {
        let kind = EffectKind::from_str(type_str)?;
        let id = self.next_effect_id;
        let limit = if target == 0 { pm_scene::MAX_GLOBAL_EFFECTS } else { pm_scene::MAX_EFFECTS_PER_LAYER };
        {
            let chain = self.chain_mut(target)?;
            if chain.len() >= limit {
                return None;
            }
            chain.add(id, kind);
        }
        self.next_effect_id += 1;
        Some(id)
    }
    pub fn remove_effect(&mut self, target: u64, id: u64) {
        if let Some(c) = self.chain_mut(target) {
            c.remove(id);
        }
    }
    pub fn duplicate_effect(&mut self, target: u64, id: u64) -> Option<u64> {
        let nid = self.next_effect_id;
        let r = self.chain_mut(target).and_then(|c| c.duplicate(id, nid));
        if r.is_some() {
            self.next_effect_id += 1;
        }
        r
    }
    pub fn move_effect(&mut self, target: u64, id: u64, up: bool) {
        if let Some(c) = self.chain_mut(target) {
            c.move_effect(id, up);
        }
    }
    pub fn set_effect_enabled(&mut self, target: u64, id: u64, enabled: bool) {
        if let Some(c) = self.chain_mut(target) {
            c.set_enabled(id, enabled);
        }
    }
    pub fn select_effect(&mut self, target: u64, id: u64) {
        if let Some(c) = self.chain_mut(target) {
            c.select(id);
        }
    }
    pub fn set_effect_param(&mut self, target: u64, id: u64, idx: usize, base: f32) {
        if let Some(c) = self.chain_mut(target) {
            c.set_param(id, idx, base);
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn set_effect_param_mod(&mut self, target: u64, id: u64, idx: usize, source: &str, amount: f32, smoothing: f32, curve: &str, invert: bool) {
        if let Some(c) = self.chain_mut(target) {
            c.set_param_mod(id, idx, source, amount, smoothing, curve, invert);
        }
    }
    pub fn reset_feedback(&mut self, target: u64) {
        if let Some(c) = self.chain_mut(target) {
            c.reset_feedback();
        }
    }
    pub fn effects_json(&self, target: u64) -> String {
        let label = if target == 0 { "global".to_string() } else { target.to_string() };
        self.chain(target).map(|c| c.to_json(&label)).unwrap_or_else(|| "{\"target\":\"none\",\"effects\":[]}".into())
    }

    /// Instantiate a built-in effect-rack preset into the target chain.
    pub fn add_effect_preset(&mut self, target: u64, preset: &str) {
        let kinds: &[&str] = match preset {
            "dreamy" => &["bloom", "chromatic", "vignette"],
            "vhs" => &["rgbsplit", "scanlines", "noise"],
            "tunnel" => &["feedback", "kaleidoscope"],
            "acid" => &["hue", "posterize", "feedback"],
            _ => &[],
        };
        for k in kinds {
            self.add_effect(target, k);
        }
    }

    fn make_runtime(&self, ctx: &GpuContext, kind: LayerKind) -> Runtime {
        match kind {
            LayerKind::Milkdrop => Runtime::Milkdrop,
            LayerKind::Shader => Runtime::Shader(ShaderState {
                shader: LiveShader::new(ctx, self.width, self.height),
                source: String::new(),
                mode: 0,
                user_slots: [[0.0; 4]; USER_SLOTS],
                user_mods: std::array::from_fn(|_| None),
                user_range: [[0.0, 1.0]; USER_SLOTS],
                controls: Vec::new(),
                attribution: Attribution::default(),
            }),
            LayerKind::Waveform => Runtime::Waveform(OverlayRenderer::new(ctx, self.width, self.height)),
            LayerKind::Spectrum => {
                let mut o = OverlayRenderer::new(ctx, self.width, self.height);
                o.cfg.mode = 2.0; // spectrum bars default
                Runtime::Spectrum(o)
            }
        }
    }

    /// Add a layer of `kind`. Milkdrop is single-instance (returns None if one
    /// already exists) — documented constraint until multi-engine support lands.
    pub fn add_layer(&mut self, ctx: &GpuContext, kind: LayerKind) -> Option<u64> {
        if kind == LayerKind::Milkdrop && self.layers.iter().any(|l| l.kind() == LayerKind::Milkdrop) {
            return None;
        }
        if self.layers.len() >= pm_scene::MAX_LAYERS {
            return None;
        }
        if kind == LayerKind::Shader
            && self.layers.iter().filter(|l| l.kind() == LayerKind::Shader).count() >= pm_scene::MAX_SHADER_LAYERS
        {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let runtime = self.make_runtime(ctx, kind);
        self.layers.push(Layer {
            id,
            name: kind.default_name().to_string(),
            enabled: true,
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            transform: Transform::default(),
            runtime,
            effects: EffectChain::default(),
            effect_output: Texture::new_render_target(&ctx.device, "layer-fx", self.width, self.height, TARGET_FORMAT),
        });
        self.selected = Some(id);
        Some(id)
    }

    pub fn remove_layer(&mut self, id: u64) {
        self.layers.retain(|l| l.id != id);
        if self.selected == Some(id) {
            self.selected = self.layers.last().map(|l| l.id);
        }
    }

    /// Duplicate a layer (shader source, controls, attribution, transform, blend
    /// all preserved). Milkdrop cannot be duplicated (single instance).
    pub fn duplicate_layer(&mut self, ctx: &GpuContext, id: u64) -> Option<u64> {
        let idx = self.layers.iter().position(|l| l.id == id)?;
        let kind = self.layers[idx].kind();
        if kind == LayerKind::Milkdrop {
            return None;
        }
        let new_id = self.add_layer(ctx, kind)?; // pushed at end + selected
        // add_layer pushed at the end; move it just after the source, then copy state.
        let new_idx = self.layers.len() - 1;
        // copy meta
        let (name, enabled, visible, opacity, blend, transform) = {
            let s = &self.layers[idx];
            (s.name.clone(), s.enabled, s.visible, s.opacity, s.blend, s.transform)
        };
        {
            let d = &mut self.layers[new_idx];
            d.name = format!("{name} copy");
            d.enabled = enabled;
            d.visible = visible;
            d.opacity = opacity;
            d.blend = blend;
            d.transform = transform;
        }
        // copy source-specific state
        self.copy_source_state(ctx, idx, new_idx);
        // place the copy right after the original
        let layer = self.layers.remove(new_idx);
        self.layers.insert(idx + 1, layer);
        self.selected = Some(new_id);
        Some(new_id)
    }

    fn copy_source_state(&mut self, ctx: &GpuContext, from: usize, to: usize) {
        // Split borrow: pull the needed data out of `from` first.
        enum Copy {
            Shader(String, u8),
            Overlay(OverlayConfig),
            None,
        }
        let payload = match &self.layers[from].runtime {
            Runtime::Shader(s) => Copy::Shader(s.source.clone(), s.mode),
            Runtime::Waveform(o) | Runtime::Spectrum(o) => Copy::Overlay(o.cfg.into()),
            Runtime::Milkdrop => Copy::None,
        };
        match (payload, &mut self.layers[to].runtime) {
            (Copy::Shader(src, mode), Runtime::Shader(d)) => {
                let m = if mode == 1 { ShaderMode::Raw } else { ShaderMode::Shadertoy };
                let outcome = d.shader.set_shader(ctx, m, &src);
                d.source = src;
                d.mode = mode;
                apply_controls(d, &outcome.controls);
            }
            (Copy::Overlay(cfg), Runtime::Waveform(o)) | (Copy::Overlay(cfg), Runtime::Spectrum(o)) => {
                o.cfg = cfg.into();
            }
            _ => {}
        }
    }

    pub fn move_layer(&mut self, id: u64, up: bool) {
        let Some(i) = self.layers.iter().position(|l| l.id == id) else { return };
        if up && i > 0 {
            self.layers.swap(i, i - 1);
        } else if !up && i + 1 < self.layers.len() {
            self.layers.swap(i, i + 1);
        }
    }

    pub fn set_selected(&mut self, id: u64) {
        if self.layers.iter().any(|l| l.id == id) {
            self.selected = Some(id);
        }
    }
    pub fn selected(&self) -> Option<u64> {
        self.selected
    }
    pub fn set_enabled(&mut self, id: u64, enabled: bool) {
        if let Some(l) = self.layer_mut(id) {
            l.enabled = enabled;
        }
    }
    pub fn set_visible(&mut self, id: u64, visible: bool) {
        if let Some(l) = self.layer_mut(id) {
            l.visible = visible;
        }
    }
    pub fn set_opacity(&mut self, id: u64, opacity: f32) {
        if let Some(l) = self.layer_mut(id) {
            l.opacity = opacity.clamp(0.0, 1.0);
        }
    }
    pub fn set_blend(&mut self, id: u64, blend: u32) {
        if let Some(l) = self.layer_mut(id) {
            l.blend = BlendMode::from_u32(blend);
        }
    }
    pub fn set_transform(&mut self, id: u64, px: f32, py: f32, sx: f32, sy: f32, rot: f32) {
        if let Some(l) = self.layer_mut(id) {
            l.transform = Transform { pos: [px, py], scale: [sx, sy], rotation: rot };
            l.transform.clamp();
        }
    }
    pub fn rename_layer(&mut self, id: u64, name: String) {
        if let Some(l) = self.layer_mut(id) {
            l.name = name;
        }
    }

    fn layer_mut(&mut self, id: u64) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// Compile a shader into the selected shader layer (or a specific one).
    pub fn set_shader(&mut self, ctx: &GpuContext, id: u64, mode: ShaderMode, src: &str) -> Option<CompileOutcome> {
        let l = self.layer_mut(id)?;
        let Runtime::Shader(s) = &mut l.runtime else { return None };
        let outcome = s.shader.set_shader(ctx, mode, src);
        if outcome.ok {
            s.source = src.to_string();
            s.mode = if mode == ShaderMode::Raw { 1 } else { 0 };
            apply_controls(s, &outcome.controls);
        }
        Some(outcome)
    }

    /// Set an overlay layer's config.
    pub fn set_overlay_cfg(&mut self, id: u64, cfg: OverlayConfig) {
        if let Some(l) = self.layer_mut(id) {
            if let Runtime::Waveform(o) | Runtime::Spectrum(o) = &mut l.runtime {
                o.cfg = cfg.into();
            }
        }
    }

    pub fn set_control(&mut self, id: u64, idx: usize, v: [f32; 4]) {
        if let Some(l) = self.layer_mut(id) {
            if let Runtime::Shader(s) = &mut l.runtime {
                if idx < USER_SLOTS {
                    s.user_slots[idx] = v;
                }
            }
        }
    }

    pub fn set_control_mod(&mut self, id: u64, idx: usize, param: Option<Parameter>) {
        if let Some(l) = self.layer_mut(id) {
            if let Runtime::Shader(s) = &mut l.runtime {
                if idx < USER_SLOTS {
                    s.user_mods[idx] = param;
                }
            }
        }
    }

    pub fn user_range(&self, id: u64, idx: usize) -> [f32; 2] {
        for l in &self.layers {
            if l.id == id {
                if let Runtime::Shader(s) = &l.runtime {
                    return s.user_range.get(idx).copied().unwrap_or([0.0, 1.0]);
                }
            }
        }
        [0.0, 1.0]
    }

    pub fn resize(&mut self, ctx: &GpuContext, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.accum_a = Texture::new_render_target(&ctx.device, "accum-a", width, height, TARGET_FORMAT);
        self.accum_b = Texture::new_render_target(&ctx.device, "accum-b", width, height, TARGET_FORMAT);
        self.effects.resize(width, height);
        self.global_output = Texture::new_render_target(&ctx.device, "global-fx", width, height, TARGET_FORMAT);
        self.global_chain.reset_feedback(); // history is size-dependent
        for l in &mut self.layers {
            match &mut l.runtime {
                Runtime::Shader(s) => s.shader.resize(ctx, width, height),
                Runtime::Waveform(o) | Runtime::Spectrum(o) => o.resize(ctx, width, height),
                Runtime::Milkdrop => {}
            }
            l.effect_output = Texture::new_render_target(&ctx.device, "layer-fx", width, height, TARGET_FORMAT);
            l.effects.reset_feedback();
        }
    }

    /// Render the whole stack for this frame and blit to `target`. `player` is
    /// the shared Milkdrop engine (already rendered by the caller); `base` holds
    /// the frame's global uniforms; `modctx` drives per-layer control modulation.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        ctx: &GpuContext,
        target: &wgpu::TextureView,
        player_output: &Texture,
        audio: &FrameAudioData,
        base: &ShaderUniforms,
        modctx: &ModContext,
        time: f32,
    ) {
        // Render each enabled+visible layer's source into its own texture.
        for l in &mut self.layers {
            if !l.enabled || !l.visible {
                continue;
            }
            match &mut l.runtime {
                Runtime::Milkdrop => {} // shared player already rendered
                Runtime::Shader(s) => {
                    let mut slots = s.user_slots;
                    for i in 0..USER_SLOTS {
                        if let Some(p) = s.user_mods[i].as_mut() {
                            p.base = s.user_slots[i][0];
                            slots[i][0] = p.eval(modctx);
                        }
                    }
                    s.shader.update_audio(ctx, audio);
                    s.shader.update_uniforms(ctx, base);
                    s.shader.update_user_controls(ctx, &slots);
                    s.shader.render(ctx);
                }
                Runtime::Waveform(o) | Runtime::Spectrum(o) => {
                    o.update_audio(ctx, audio);
                    o.render(ctx);
                }
            }
        }

        // Apply per-layer effect chains (source texture → the layer's effect_output).
        self.effects.begin_frame();
        for i in 0..self.layers.len() {
            let layer = &mut self.layers[i];
            if !layer.enabled || !layer.visible || layer.effects.is_empty_enabled() {
                continue;
            }
            let source_tex: &Texture = match &layer.runtime {
                Runtime::Milkdrop => player_output,
                Runtime::Shader(s) => s.shader.output(),
                Runtime::Waveform(o) | Runtime::Spectrum(o) => o.output(),
            };
            self.effects.apply(ctx, &mut layer.effects, source_tex, &layer.effect_output, modctx, time);
        }

        // Clear the first accumulator to opaque black.
        clear_black(ctx, &self.accum_a.view);
        let mut read_is_a = true;

        for i in 0..self.layers.len() {
            let (enabled, visible, opacity, blend, transform) = {
                let l = &self.layers[i];
                (l.enabled, l.visible, l.opacity, l.blend, l.transform)
            };
            if !enabled || !visible {
                continue;
            }
            let opaque = matches!(self.layers[i].kind(), LayerKind::Milkdrop | LayerKind::Shader);
            let has_fx = !self.layers[i].effects.is_empty_enabled();
            let src_view: &wgpu::TextureView = if has_fx {
                &self.layers[i].effect_output.view
            } else {
                match &self.layers[i].runtime {
                    Runtime::Milkdrop => &player_output.view,
                    Runtime::Shader(s) => &s.shader.output().view,
                    Runtime::Waveform(o) | Runtime::Spectrum(o) => &o.output().view,
                }
            };

            let (read, write) = if read_is_a {
                (&self.accum_a, &self.accum_b)
            } else {
                (&self.accum_b, &self.accum_a)
            };
            self.composite(ctx, read, src_view, write, opacity, blend, transform, opaque);
            read_is_a = !read_is_a;
        }

        // Global effects on the composited scene, then blit.
        let final_tex = if read_is_a { &self.accum_a } else { &self.accum_b };
        if self.global_chain.is_empty_enabled() {
            self.blit.draw(ctx, final_tex, target);
        } else {
            self.effects.apply(ctx, &mut self.global_chain, final_tex, &self.global_output, modctx, time);
            self.blit.draw(ctx, &self.global_output, target);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn composite(
        &self,
        ctx: &GpuContext,
        read: &Texture,
        src: &wgpu::TextureView,
        write: &Texture,
        opacity: f32,
        blend: BlendMode,
        transform: Transform,
        opaque: bool,
    ) {
        let (c, s) = (transform.rotation.cos(), transform.rotation.sin());
        let (sx, sy) = (transform.scale[0].max(1e-4), transform.scale[1].max(1e-4));
        let u = CompUniform {
            m: [c / sx, s / sx, -s / sy, c / sy],
            pos: transform.pos,
            resolution: [self.width as f32, self.height as f32],
            opacity,
            blend: blend.as_u32() as f32,
            opaque: if opaque { 1.0 } else { 0.0 },
            _pad: 0.0,
        };
        ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));

        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&read.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("composite enc") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &write.view,
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
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }

    // --- Introspection / scene serialization -------------------------------

    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
    pub fn enabled_count(&self) -> usize {
        self.layers.iter().filter(|l| l.enabled && l.visible).count()
    }
    pub fn shader_count(&self) -> usize {
        self.layers.iter().filter(|l| l.kind() == LayerKind::Shader).count()
    }
    pub fn has_milkdrop(&self) -> bool {
        self.layers.iter().any(|l| l.kind() == LayerKind::Milkdrop)
    }

    /// A JSON description of the stack for the UI (id/name/kind/flags per layer).
    pub fn layers_json(&self) -> String {
        let items: Vec<String> = self
            .layers
            .iter()
            .map(|l| {
                format!(
                    "{{\"id\":{},\"name\":{},\"kind\":\"{}\",\"enabled\":{},\"visible\":{},\"opacity\":{:.3},\"blend\":{},\"selected\":{}}}",
                    l.id,
                    json_str(&l.name),
                    l.kind_str(),
                    l.enabled,
                    l.visible,
                    l.opacity,
                    l.blend.as_u32(),
                    self.selected == Some(l.id),
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// The selected shader layer's controls as JSON (for the editor panel).
    pub fn selected_controls_json(&self) -> String {
        if let Some(id) = self.selected {
            if let Some(l) = self.layers.iter().find(|l| l.id == id) {
                if let Runtime::Shader(s) = &l.runtime {
                    let cs: Vec<String> = s.controls.iter().map(control_json).collect();
                    return format!("{{\"source\":{},\"mode\":{},\"controls\":[{}]}}", json_str(&s.source), s.mode, cs.join(","));
                }
            }
        }
        "{\"source\":\"\",\"mode\":0,\"controls\":[]}".to_string()
    }

    pub fn export_scene(&self, speed: f32, paused: bool, bpm: f32, tempo_manual: bool, subdivision: f32) -> SceneData {
        let layers = self
            .layers
            .iter()
            .map(|l| LayerData {
                id: l.id,
                name: l.name.clone(),
                enabled: l.enabled,
                visible: l.visible,
                opacity: l.opacity,
                blend: l.blend,
                transform: l.transform,
                source: match &l.runtime {
                    Runtime::Milkdrop => SourceState::Milkdrop,
                    Runtime::Shader(s) => SourceState::Shader {
                        source: s.source.clone(),
                        mode: s.mode,
                        controls: s.user_slots.to_vec(),
                        mods: collect_mods(s),
                        attribution: s.attribution.clone(),
                    },
                    Runtime::Waveform(o) => SourceState::Waveform(o.cfg.into()),
                    Runtime::Spectrum(o) => SourceState::Spectrum(o.cfg.into()),
                },
                effects: l.effects.to_data(),
            })
            .collect();
        SceneData {
            schema_version: SCHEMA_VERSION,
            scene_id: "scene".into(),
            name: "Scene".into(),
            layers,
            speed,
            paused,
            bpm,
            tempo_manual,
            subdivision,
            global_effects: self.global_chain.to_data(),
        }
    }

    /// Build a fresh stack from a validated scene (transactional — the caller
    /// only swaps this in after we return Ok, so a bad import keeps the old one).
    pub fn import_scene(&mut self, ctx: &GpuContext, scene: &SceneData) {
        self.layers.clear();
        self.next_id = 1;
        for ld in &scene.layers {
            let kind = match &ld.source {
                SourceState::Milkdrop => LayerKind::Milkdrop,
                SourceState::Shader { .. } => LayerKind::Shader,
                SourceState::Waveform(_) => LayerKind::Waveform,
                SourceState::Spectrum(_) => LayerKind::Spectrum,
            };
            // Skip a second Milkdrop (single-instance constraint).
            if kind == LayerKind::Milkdrop && self.has_milkdrop() {
                continue;
            }
            let mut runtime = self.make_runtime(ctx, kind);
            match (&ld.source, &mut runtime) {
                (SourceState::Shader { source, mode, controls, mods, attribution }, Runtime::Shader(s)) => {
                    let m = if *mode == 1 { ShaderMode::Raw } else { ShaderMode::Shadertoy };
                    let outcome = s.shader.set_shader(ctx, m, source);
                    s.source = source.clone();
                    s.mode = *mode;
                    s.attribution = attribution.clone();
                    apply_controls(s, &outcome.controls);
                    for (i, c) in controls.iter().enumerate().take(USER_SLOTS) {
                        s.user_slots[i] = *c;
                    }
                    for mm in mods {
                        let i = mm.slot as usize;
                        if i < USER_SLOTS {
                            let [mn, mx] = s.user_range[i];
                            let mut p = Parameter::new(s.user_slots[i][0], mn, mx);
                            p.source = pm_params::ModSource::from_str(&mm.source);
                            p.amount = mm.amount;
                            p.smoothing = mm.smoothing.clamp(0.0, 0.999);
                            s.user_mods[i] = Some(p);
                        }
                    }
                }
                (SourceState::Waveform(cfg), Runtime::Waveform(o))
                | (SourceState::Spectrum(cfg), Runtime::Spectrum(o)) => o.cfg = cfg.clone().into(),
                _ => {}
            }
            let id = self.next_id;
            self.next_id += 1;
            let mut chain = EffectChain::default();
            {
                let next = &mut self.next_effect_id;
                chain.from_data(&ld.effects, &mut || {
                    let eid = *next;
                    *next += 1;
                    eid
                });
            }
            self.layers.push(Layer {
                id,
                name: ld.name.clone(),
                enabled: ld.enabled,
                visible: ld.visible,
                opacity: ld.opacity.clamp(0.0, 1.0),
                blend: ld.blend,
                transform: ld.transform,
                runtime,
                effects: chain,
                effect_output: Texture::new_render_target(&ctx.device, "layer-fx", self.width, self.height, TARGET_FORMAT),
            });
        }
        // Rebuild global effects (feedback history starts clean).
        {
            let next = &mut self.next_effect_id;
            self.global_chain.from_data(&scene.global_effects, &mut || {
                let eid = *next;
                *next += 1;
                eid
            });
        }
        self.selected = self.layers.first().map(|l| l.id);
    }
}

impl Layer {
    fn kind_str(&self) -> &'static str {
        match self.runtime {
            Runtime::Milkdrop => "milkdrop",
            Runtime::Shader(_) => "shader",
            Runtime::Waveform(_) => "waveform",
            Runtime::Spectrum(_) => "spectrum",
        }
    }
}

fn apply_controls(s: &mut ShaderState, controls: &[Control]) {
    s.user_slots = [[0.0; 4]; USER_SLOTS];
    s.user_mods = std::array::from_fn(|_| None);
    s.user_range = [[0.0, 1.0]; USER_SLOTS];
    for c in controls {
        let slot = c.slot as usize;
        if slot < USER_SLOTS {
            s.user_slots[slot] = c.default;
            s.user_range[slot] = [c.min, c.max];
        }
    }
    s.controls = controls.to_vec();
}

fn collect_mods(s: &ShaderState) -> Vec<ModMapping> {
    s.user_mods
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.as_ref().map(|p| ModMapping { slot: i as u32, source: mod_source_str(p.source), amount: p.amount, smoothing: p.smoothing }))
        .collect()
}

fn mod_source_str(s: pm_params::ModSource) -> String {
    use pm_params::ModSource::*;
    match s {
        Bass => "bass", Mid => "mid", Treb => "treb", Vol => "vol",
        BassAtt => "bassAtt", MidAtt => "midAtt", TrebAtt => "trebAtt", VolAtt => "volAtt",
        BeatPulse => "beatPulse", BeatPhase => "beatPhase",
        Lfo(0) => "lfo0", Lfo(1) => "lfo1", Lfo(2) => "lfo2", Lfo(_) => "lfo3",
        None => "none",
    }
    .to_string()
}

fn control_json(c: &Control) -> String {
    let opts: Vec<String> = c.options.iter().map(|o| json_str(o)).collect();
    format!(
        "{{\"name\":{},\"kind\":\"{}\",\"min\":{},\"max\":{},\"slot\":{},\"default\":[{},{},{},{}],\"options\":[{}]}}",
        json_str(&c.name), c.kind.as_str(), c.min, c.max, c.slot,
        c.default[0], c.default[1], c.default[2], c.default[3], opts.join(",")
    )
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn clear_black(ctx: &GpuContext, view: &wgpu::TextureView) {
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("accum clear") });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("accum clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    ctx.queue.submit(Some(encoder.finish()));
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
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
