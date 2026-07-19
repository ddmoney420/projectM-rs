//! Reorderable effect chains (per-layer and global) built on the compositor.
//!
//! Most effects are a single fullscreen pass through one shared "übershader"
//! pipeline selected by a `mode` uniform (so live parameter changes never
//! recompile anything). Blur is separable (two passes), bloom is multipass at
//! reduced resolution, and feedback is stateful (owns a previous-frame texture).
//! A bounded render-target pool supplies transient textures; nothing allocates a
//! full-res texture per effect per frame beyond the pool's reused set.
//!
//! ALL sampling uses `textureSampleLevel(.., 0.0)` — no implicit derivatives —
//! so multi-tap loops and mode branches never hit WGSL's uniform-control-flow
//! rule (the class of bug that produced the black Phase 6 composite).

use pm_params::{Curve, ModContext, ModSource, Parameter};
use pm_render::{GpuContext, Texture, TARGET_FORMAT};
use pm_scene::{EffectData, ParamData};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectKind {
    Brightness,
    Contrast,
    Saturation,
    Hue,
    Invert,
    Posterize,
    MirrorH,
    MirrorV,
    Kaleidoscope,
    Radial,
    Pixelate,
    Blur,
    Sharpen,
    Edge,
    Vignette,
    Noise,
    Scanlines,
    Chromatic,
    RgbSplit,
    Glitch,
    Feedback,
    Bloom,
}

/// (name, min, max, default) for each parameter, in order.
type ParamDef = (&'static str, f32, f32, f32);

impl EffectKind {
    pub fn type_str(self) -> &'static str {
        use EffectKind::*;
        match self {
            Brightness => "brightness", Contrast => "contrast", Saturation => "saturation",
            Hue => "hue", Invert => "invert", Posterize => "posterize",
            MirrorH => "mirrorh", MirrorV => "mirrorv", Kaleidoscope => "kaleidoscope",
            Radial => "radial", Pixelate => "pixelate", Blur => "blur", Sharpen => "sharpen",
            Edge => "edge", Vignette => "vignette", Noise => "noise", Scanlines => "scanlines",
            Chromatic => "chromatic", RgbSplit => "rgbsplit", Glitch => "glitch",
            Feedback => "feedback", Bloom => "bloom",
        }
    }

    pub fn from_str(s: &str) -> Option<EffectKind> {
        use EffectKind::*;
        Some(match s {
            "brightness" => Brightness, "contrast" => Contrast, "saturation" => Saturation,
            "hue" => Hue, "invert" => Invert, "posterize" => Posterize,
            "mirrorh" => MirrorH, "mirrorv" => MirrorV, "kaleidoscope" => Kaleidoscope,
            "radial" => Radial, "pixelate" => Pixelate, "blur" => Blur, "sharpen" => Sharpen,
            "edge" => Edge, "vignette" => Vignette, "noise" => Noise, "scanlines" => Scanlines,
            "chromatic" => Chromatic, "rgbsplit" => RgbSplit, "glitch" => Glitch,
            "feedback" => Feedback, "bloom" => Bloom,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        use EffectKind::*;
        match self {
            Brightness => "Brightness", Contrast => "Contrast", Saturation => "Saturation",
            Hue => "Hue rotate", Invert => "Invert", Posterize => "Posterize",
            MirrorH => "Mirror H", MirrorV => "Mirror V", Kaleidoscope => "Kaleidoscope",
            Radial => "Radial symmetry", Pixelate => "Pixelate", Blur => "Blur", Sharpen => "Sharpen",
            Edge => "Edge detect", Vignette => "Vignette", Noise => "Noise", Scanlines => "Scanlines",
            Chromatic => "Chromatic aberration", RgbSplit => "RGB split", Glitch => "Glitch",
            Feedback => "Feedback", Bloom => "Bloom",
        }
    }

    fn mode(self) -> u32 {
        use EffectKind::*;
        match self {
            Brightness => 0, Contrast => 1, Saturation => 2, Hue => 3, Invert => 4, Posterize => 5,
            MirrorH => 6, MirrorV => 7, Kaleidoscope => 8, Radial => 9, Pixelate => 10,
            Blur => 11, Sharpen => 12, Edge => 13, Vignette => 14, Noise => 15, Scanlines => 16,
            Chromatic => 17, RgbSplit => 18, Glitch => 19, Feedback => 20,
            // Bloom is composed of modes 21 (bright) + 11 (blur) + 22 (combine).
            Bloom => 21,
        }
    }

    pub fn params(self) -> &'static [ParamDef] {
        use EffectKind::*;
        match self {
            Brightness => &[("amount", -1.0, 1.0, 0.0)],
            Contrast => &[("amount", 0.0, 2.0, 1.0)],
            Saturation => &[("amount", 0.0, 2.0, 1.0)],
            Hue => &[("shift", 0.0, 1.0, 0.0)],
            Invert => &[("mix", 0.0, 1.0, 1.0)],
            Posterize => &[("levels", 2.0, 16.0, 6.0)],
            MirrorH | MirrorV => &[],
            Kaleidoscope => &[("segments", 2.0, 16.0, 6.0), ("rotation", 0.0, 6.283, 0.0)],
            Radial => &[("segments", 2.0, 16.0, 4.0), ("rotation", 0.0, 6.283, 0.0)],
            Pixelate => &[("size", 2.0, 256.0, 64.0)],
            Blur => &[("radius", 0.0, 16.0, 4.0)],
            Sharpen => &[("amount", 0.0, 3.0, 1.0)],
            Edge => &[("strength", 0.0, 4.0, 1.0)],
            Vignette => &[("amount", 0.0, 2.0, 0.8), ("softness", 0.01, 1.0, 0.5)],
            Noise => &[("amount", 0.0, 1.0, 0.15)],
            Scanlines => &[("amount", 0.0, 1.0, 0.4), ("count", 100.0, 2000.0, 800.0)],
            Chromatic => &[("amount", 0.0, 0.05, 0.005)],
            RgbSplit => &[("amount", 0.0, 0.05, 0.01), ("angle", 0.0, 6.283, 0.0)],
            Glitch => &[("amount", 0.0, 1.0, 0.3)],
            Feedback => &[
                ("amount", 0.0, 1.0, 0.9), ("zoom", 0.9, 1.1, 1.01), ("rotation", -0.1, 0.1, 0.0),
                ("offsetX", -0.1, 0.1, 0.0), ("offsetY", -0.1, 0.1, 0.0),
            ],
            Bloom => &[("threshold", 0.0, 1.0, 0.6), ("intensity", 0.0, 3.0, 1.0), ("radius", 0.0, 16.0, 6.0)],
        }
    }

}

fn curve_from(s: &str) -> Curve {
    match s {
        "exp" => Curve::Exp,
        "log" => Curve::Log,
        "scurve" => Curve::SCurve,
        _ => Curve::Linear,
    }
}
fn curve_str(c: Curve) -> &'static str {
    match c {
        Curve::Exp => "exp",
        Curve::Log => "log",
        Curve::SCurve => "scurve",
        Curve::Linear => "linear",
    }
}
fn mod_source_str(s: ModSource) -> &'static str {
    use ModSource::*;
    match s {
        Bass => "bass", Mid => "mid", Treb => "treb", Vol => "vol",
        BassAtt => "bassAtt", MidAtt => "midAtt", TrebAtt => "trebAtt", VolAtt => "volAtt",
        BeatPulse => "beatPulse", BeatPhase => "beatPhase",
        Lfo(0) => "lfo0", Lfo(1) => "lfo1", Lfo(2) => "lfo2", Lfo(_) => "lfo3",
        None => "none",
    }
}

/// One effect instance.
pub struct Effect {
    pub id: u64,
    pub name: String,
    pub kind: EffectKind,
    pub enabled: bool,
    params: Vec<Parameter>,
    /// Feedback history (ping-pong); independent per instance.
    feedback: Option<[Texture; 2]>,
    fb_which: usize,
    fb_primed: bool,
}

impl Effect {
    fn new(id: u64, kind: EffectKind) -> Self {
        let params = kind
            .params()
            .iter()
            .map(|(_, min, max, def)| {
                let mut p = Parameter::new(*def, *min, *max);
                p.base = *def;
                p
            })
            .collect();
        Effect { id, name: kind.name().to_string(), kind, enabled: true, params, feedback: None, fb_which: 0, fb_primed: false }
    }

    fn eval_params(&mut self, ctx: &ModContext) -> [f32; 6] {
        let mut out = [0.0f32; 6];
        for (i, p) in self.params.iter_mut().enumerate().take(6) {
            out[i] = p.eval(ctx);
        }
        out
    }

    fn to_data(&self) -> EffectData {
        let params = self
            .params
            .iter()
            .map(|p| ParamData {
                base: p.base,
                source: mod_source_str(p.source).to_string(),
                amount: p.amount,
                smoothing: p.smoothing,
                curve: curve_str(p.curve).to_string(),
                invert: p.invert,
            })
            .collect();
        EffectData {
            id: self.id,
            name: self.name.clone(),
            effect_type: self.kind.type_str().to_string(),
            enabled: self.enabled,
            params,
        }
    }

    fn apply_data(&mut self, d: &EffectData) {
        self.name = d.name.clone();
        self.enabled = d.enabled;
        for (i, pd) in d.params.iter().enumerate() {
            if let Some(p) = self.params.get_mut(i) {
                p.base = pd.base.clamp(p.min, p.max);
                p.source = ModSource::from_str(&pd.source);
                p.amount = pd.amount;
                p.smoothing = pd.smoothing.clamp(0.0, 0.999);
                p.curve = curve_from(&pd.curve);
                p.invert = pd.invert;
            }
        }
    }
}

/// A reorderable chain of effects (a layer's chain or the global chain).
#[derive(Default)]
pub struct EffectChain {
    effects: Vec<Effect>,
    selected: Option<u64>,
}

impl EffectChain {
    pub fn is_empty_enabled(&self) -> bool {
        !self.effects.iter().any(|e| e.enabled)
    }
    pub fn len(&self) -> usize {
        self.effects.len()
    }
    pub fn add(&mut self, id: u64, kind: EffectKind) -> u64 {
        self.effects.push(Effect::new(id, kind));
        self.selected = Some(id);
        id
    }
    pub fn remove(&mut self, id: u64) {
        self.effects.retain(|e| e.id != id);
        if self.selected == Some(id) {
            self.selected = self.effects.last().map(|e| e.id);
        }
    }
    pub fn duplicate(&mut self, id: u64, new_id: u64) -> Option<u64> {
        let idx = self.effects.iter().position(|e| e.id == id)?;
        let mut copy = Effect::new(new_id, self.effects[idx].kind);
        copy.name = format!("{} copy", self.effects[idx].name);
        copy.enabled = self.effects[idx].enabled;
        for (i, p) in self.effects[idx].params.iter().enumerate() {
            if let Some(dp) = copy.params.get_mut(i) {
                dp.base = p.base;
                dp.source = p.source;
                dp.amount = p.amount;
                dp.smoothing = p.smoothing;
                dp.curve = p.curve;
                dp.invert = p.invert;
            }
        }
        // A duplicated feedback effect gets its OWN fresh history (feedback is None).
        self.effects.insert(idx + 1, copy);
        self.selected = Some(new_id);
        Some(new_id)
    }
    pub fn move_effect(&mut self, id: u64, up: bool) {
        let Some(i) = self.effects.iter().position(|e| e.id == id) else { return };
        if up && i > 0 {
            self.effects.swap(i, i - 1);
        } else if !up && i + 1 < self.effects.len() {
            self.effects.swap(i, i + 1);
        }
    }
    pub fn set_enabled(&mut self, id: u64, enabled: bool) {
        if let Some(e) = self.effects.iter_mut().find(|e| e.id == id) {
            e.enabled = enabled;
        }
    }
    pub fn select(&mut self, id: u64) {
        if self.effects.iter().any(|e| e.id == id) {
            self.selected = Some(id);
        }
    }
    pub fn set_param(&mut self, id: u64, idx: usize, base: f32) {
        if let Some(e) = self.effects.iter_mut().find(|e| e.id == id) {
            if let Some(p) = e.params.get_mut(idx) {
                p.base = base.clamp(p.min, p.max);
            }
        }
    }
    pub fn set_param_mod(&mut self, id: u64, idx: usize, source: &str, amount: f32, smoothing: f32, curve: &str, invert: bool) {
        if let Some(e) = self.effects.iter_mut().find(|e| e.id == id) {
            if let Some(p) = e.params.get_mut(idx) {
                p.source = ModSource::from_str(source);
                p.amount = amount;
                p.smoothing = smoothing.clamp(0.0, 0.999);
                p.curve = curve_from(curve);
                p.invert = invert;
            }
        }
    }
    /// Reset feedback history on all feedback effects in this chain.
    pub fn reset_feedback(&mut self) {
        for e in &mut self.effects {
            e.feedback = None;
            e.fb_primed = false;
        }
    }

    pub fn to_data(&self) -> Vec<EffectData> {
        self.effects.iter().map(Effect::to_data).collect()
    }

    // --- MIDI target introspection (stable ids) ----------------------------

    /// `(id, kind, enabled)` for each effect, in order — used to enumerate
    /// MIDI-mappable effect targets.
    pub fn ids_kinds(&self) -> Vec<(u64, EffectKind, bool)> {
        self.effects.iter().map(|e| (e.id, e.kind, e.enabled)).collect()
    }
    /// Live base value of an effect parameter (the MIDI-controlled value).
    pub fn param_base(&self, id: u64, idx: usize) -> Option<f32> {
        self.effects.iter().find(|e| e.id == id).and_then(|e| e.params.get(idx)).map(|p| p.base)
    }
    /// `[min, max]` for an effect parameter (from the effect type definition).
    pub fn param_range(&self, id: u64, idx: usize) -> Option<[f32; 2]> {
        self.effects.iter().find(|e| e.id == id).and_then(|e| e.params.get(idx)).map(|p| [p.min, p.max])
    }
    pub fn enabled(&self, id: u64) -> Option<bool> {
        self.effects.iter().find(|e| e.id == id).map(|e| e.enabled)
    }
    pub fn has(&self, id: u64) -> bool {
        self.effects.iter().any(|e| e.id == id)
    }

    /// Rebuild the chain from serialized data, **preserving** each effect's
    /// stored id so it is a stable address (e.g. for MIDI mappings) across a
    /// save/reload. Ids that are 0 or collide within the chain get a fresh
    /// fallback id. Unknown effect types are skipped. Returns the highest id
    /// used, so the caller can advance its global id counter past it.
    pub fn from_data(&mut self, data: &[EffectData]) -> u64 {
        self.effects.clear();
        let mut used: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut fallback = 1u64;
        let mut max_id = 0u64;
        for d in data {
            if let Some(kind) = EffectKind::from_str(&d.effect_type) {
                let mut id = d.id;
                if id == 0 || used.contains(&id) {
                    while used.contains(&fallback) {
                        fallback += 1;
                    }
                    id = fallback;
                }
                used.insert(id);
                max_id = max_id.max(id);
                let mut e = Effect::new(id, kind);
                e.apply_data(d);
                self.effects.push(e);
            }
        }
        self.selected = self.effects.first().map(|e| e.id);
        max_id
    }

    pub fn to_json(&self, chain_target: &str) -> String {
        let items: Vec<String> = self
            .effects
            .iter()
            .map(|e| {
                let params: Vec<String> = e
                    .params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let (name, _, _, _) = e.kind.params()[i];
                        format!(
                            "{{\"name\":\"{}\",\"min\":{},\"max\":{},\"base\":{},\"source\":\"{}\",\"amount\":{}}}",
                            name, p.min, p.max, p.base, mod_source_str(p.source), p.amount
                        )
                    })
                    .collect();
                format!(
                    "{{\"id\":{},\"name\":\"{}\",\"type\":\"{}\",\"enabled\":{},\"selected\":{},\"params\":[{}]}}",
                    e.id,
                    e.name,
                    e.kind.type_str(),
                    e.enabled,
                    self.selected == Some(e.id),
                    params.join(",")
                )
            })
            .collect();
        format!("{{\"target\":\"{}\",\"effects\":[{}]}}", chain_target, items.join(","))
    }
}

// ---------------------------------------------------------------------------
// GPU: shared übershader pipeline + render-target pool.
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct EffectUniform {
    resolution: [f32; 2],
    mode: f32,
    time: f32,
    p: [f32; 4],
    p2: [f32; 2],
    pad: [f32; 2],
}

const EFFECT_WGSL: &str = include_str!("effects.wgsl");

struct Pool {
    textures: Vec<Texture>,
    used: Vec<bool>,
    w: u32,
    h: u32,
}

impl Pool {
    fn new(w: u32, h: u32) -> Self {
        Pool { textures: Vec::new(), used: Vec::new(), w, h }
    }
    fn reset(&mut self) {
        self.used.iter_mut().for_each(|u| *u = false);
    }
    fn resize(&mut self, w: u32, h: u32) {
        self.textures.clear();
        self.used.clear();
        self.w = w;
        self.h = h;
    }
    fn acquire(&mut self, ctx: &GpuContext) -> usize {
        if let Some(i) = self.used.iter().position(|u| !u) {
            self.used[i] = true;
            return i;
        }
        self.textures
            .push(Texture::new_render_target(&ctx.device, "fx-pool", self.w, self.h, TARGET_FORMAT));
        self.used.push(true);
        self.textures.len() - 1
    }
}

/// Shared effect GPU resources (one pipeline, a sampler, a pool).
pub struct Effects {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    dummy: Texture,
    pool: Pool,
    width: u32,
    height: u32,
}

impl Effects {
    pub fn new(ctx: &GpuContext, width: u32, height: u32) -> Self {
        let device = &ctx.device;
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx bgl"),
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
            label: Some("fx shader"),
            source: wgpu::ShaderSource::Wgsl(EFFECT_WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fx pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState { format: TARGET_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fx sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fx uniform"),
            size: std::mem::size_of::<EffectUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dummy = Texture::new_render_target(device, "fx-dummy", 1, 1, TARGET_FORMAT);
        Effects { pipeline, bgl, sampler, uniform_buf, dummy, pool: Pool::new(width, height), width, height }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.pool.resize(width, height);
    }

    /// Call once per frame before applying any chains.
    pub fn begin_frame(&mut self) {
        self.pool.reset();
    }

    /// Run `chain` over `input`, writing the final result into `output`. If no
    /// enabled effects, copies `input` → `output`.
    pub fn apply(&mut self, ctx: &GpuContext, chain: &mut EffectChain, input: &Texture, output: &Texture, modctx: &ModContext, time: f32) {
        // Pre-acquire all scratch up front (the only `&mut pool` step) so the
        // per-effect loop can use `&self` methods without borrow conflicts.
        let a = self.pool.acquire(ctx);
        let b = self.pool.acquire(ctx);
        let bright = self.pool.acquire(ctx);
        let tmp = self.pool.acquire(ctx);

        let mut read_ext = true;
        let mut cur = a;
        let mut prev = b;
        let mut any = false;

        for ei in 0..chain.effects.len() {
            if !chain.effects[ei].enabled {
                continue;
            }
            let params = chain.effects[ei].eval_params(modctx);
            let kind = chain.effects[ei].kind;
            let iv: &wgpu::TextureView = if read_ext { &input.view } else { &self.pool.textures[prev].view };

            match kind {
                EffectKind::Feedback => self.run_feedback(ctx, &mut chain.effects[ei], iv, cur, &params, time),
                EffectKind::Bloom => self.run_bloom(ctx, iv, cur, bright, tmp, &params, time),
                EffectKind::Blur => {
                    // Separable: H into `tmp`, then V into `cur`.
                    self.run_pass(ctx, 11, iv, None, tmp, &params, time, 0.0);
                    self.run_pass(ctx, 11, &self.pool.textures[tmp].view, None, cur, &params, time, 1.0);
                }
                other => self.run_pass(ctx, other.mode(), iv, None, cur, &params, time, 0.0),
            }

            read_ext = false;
            std::mem::swap(&mut cur, &mut prev);
            any = true;
        }

        if any {
            copy_tex(ctx, &self.pool.textures[prev], output);
        } else {
            copy_tex(ctx, input, output);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_pass(&self, ctx: &GpuContext, mode: u32, input_view: &wgpu::TextureView, aux_view: Option<&wgpu::TextureView>, dst_idx: usize, params: &[f32; 6], time: f32, blur_dir: f32) {
        let u = EffectUniform {
            resolution: [self.pool.w as f32, self.pool.h as f32],
            mode: mode as f32,
            time,
            p: [params[0], params[1], params[2], params[3]],
            p2: [params[4], if mode == 11 { blur_dir } else { params[5] }],
            pad: [0.0, 0.0],
        };
        ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));
        let aux = aux_view.unwrap_or(&self.dummy.view);
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fx bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(input_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(aux) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let target = &self.pool.textures[dst_idx].view;
        draw(ctx, &self.pipeline, &bind, target);
    }

    #[allow(clippy::too_many_arguments)]
    fn run_feedback(&self, ctx: &GpuContext, effect: &mut Effect, input_view: &wgpu::TextureView, dst_idx: usize, params: &[f32; 6], time: f32) {
        // Ensure history textures exist and are cleared on first use / resize.
        if effect.feedback.is_none() {
            effect.feedback = Some([
                Texture::new_render_target(&ctx.device, "fb0", self.width, self.height, TARGET_FORMAT),
                Texture::new_render_target(&ctx.device, "fb1", self.width, self.height, TARGET_FORMAT),
            ]);
            effect.fb_primed = false;
        }
        if !effect.fb_primed {
            let fb = effect.feedback.as_ref().unwrap();
            clear_black(ctx, &fb[0].view);
            clear_black(ctx, &fb[1].view);
            effect.fb_primed = true;
        }
        // Read current (input) + previous history (aux); write to dst scratch.
        {
            let fb = effect.feedback.as_ref().unwrap();
            let prev = &fb[effect.fb_which].view;
            self.run_pass(ctx, EffectKind::Feedback.mode(), input_view, Some(prev), dst_idx, params, time, 0.0);
        }
        // Store the result as next frame's history, then flip.
        let next = 1 - effect.fb_which;
        copy_tex(ctx, &self.pool.textures[dst_idx], &effect.feedback.as_ref().unwrap()[next]);
        effect.fb_which = next;
    }

    #[allow(clippy::too_many_arguments)]
    fn run_bloom(&self, ctx: &GpuContext, input_view: &wgpu::TextureView, dst_idx: usize, bright: usize, tmp: usize, params: &[f32; 6], time: f32) {
        // bright-pass (threshold p0, intensity p1) → blur H → blur V → combine.
        self.run_pass(ctx, 21, input_view, None, bright, params, time, 0.0);
        let blur_params = [params[2], 0.0, 0.0, 0.0, 0.0, 0.0];
        self.run_pass(ctx, 11, &self.pool.textures[bright].view, None, tmp, &blur_params, time, 0.0);
        self.run_pass(ctx, 11, &self.pool.textures[tmp].view, None, bright, &blur_params, time, 1.0);
        let combine = [params[1], 0.0, 0.0, 0.0, 0.0, 0.0];
        self.run_pass(ctx, 22, input_view, Some(&self.pool.textures[bright].view), dst_idx, &combine, time, 0.0);
    }
}

fn copy_tex(ctx: &GpuContext, from: &Texture, to: &Texture) {
    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fx copy") });
    let w = from.width.min(to.width);
    let h = from.height.min(to.height);
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo { texture: &from.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyTextureInfo { texture: &to.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    ctx.queue.submit(Some(encoder.finish()));
}

fn draw(ctx: &GpuContext, pipeline: &wgpu::RenderPipeline, bind: &wgpu::BindGroup, target: &wgpu::TextureView) {
    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fx enc") });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fx pass"),
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
    ctx.queue.submit(Some(encoder.finish()));
}

fn clear_black(ctx: &GpuContext, view: &wgpu::TextureView) {
    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fb clear") });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("fb clear"),
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
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    }
}
