//! Serializable layer/scene data model for the pm-web compositor, plus the
//! blend math (mirrored in the compositor's WGSL) and scene validation. Pure
//! logic, unit-tested on native — no GPU. The runtime compositor in pm-web maps
//! its live layers to/from these types for persistence and scene import/export.
//!
//! Compositing model: the canvas is an **opaque** accumulator; each layer blends
//! onto it by an effective alpha `a = src.alpha * opacity`. `Normal`/`Screen`/
//! `Multiply`/`Difference`/`Lighten`/`Darken` compute a blended color then
//! `mix(dst, blended, a)`; `Add` is `dst + src*a`. Straight (non-premultiplied)
//! alpha, values clamped to `[0,1]`.

use serde::{Deserialize, Serialize};

/// Current scene schema version. Bump on incompatible changes.
pub const SCHEMA_VERSION: u32 = 1;
/// Safety limits (documented; enforced by [`SceneData::validate`]).
pub const MAX_LAYERS: usize = 16;
pub const MAX_SHADER_LAYERS: usize = 8;
pub const MAX_SHADER_SOURCE: usize = 64 * 1024;
pub const MAX_SCENE_BYTES: usize = 1_000_000;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum BlendMode {
    #[default]
    Normal,
    Add,
    Screen,
    Multiply,
    Difference,
    Lighten,
    Darken,
}

impl BlendMode {
    pub fn as_u32(self) -> u32 {
        match self {
            BlendMode::Normal => 0,
            BlendMode::Add => 1,
            BlendMode::Screen => 2,
            BlendMode::Multiply => 3,
            BlendMode::Difference => 4,
            BlendMode::Lighten => 5,
            BlendMode::Darken => 6,
        }
    }
    pub fn from_u32(v: u32) -> BlendMode {
        match v {
            1 => BlendMode::Add,
            2 => BlendMode::Screen,
            3 => BlendMode::Multiply,
            4 => BlendMode::Difference,
            5 => BlendMode::Lighten,
            6 => BlendMode::Darken,
            _ => BlendMode::Normal,
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Reference blend used for tests; the WGSL compositor mirrors this exactly.
/// `dst` is the opaque accumulator RGB, `src` the layer RGBA, `opacity` 0..1.
pub fn blend(mode: BlendMode, dst: [f32; 3], src: [f32; 4], opacity: f32) -> [f32; 3] {
    let a = (src[3] * opacity).clamp(0.0, 1.0);
    let s = [src[0], src[1], src[2]];
    let b = match mode {
        BlendMode::Normal => s,
        BlendMode::Add => {
            return [
                (dst[0] + s[0] * a).clamp(0.0, 1.0),
                (dst[1] + s[1] * a).clamp(0.0, 1.0),
                (dst[2] + s[2] * a).clamp(0.0, 1.0),
            ];
        }
        BlendMode::Screen => [
            1.0 - (1.0 - dst[0]) * (1.0 - s[0]),
            1.0 - (1.0 - dst[1]) * (1.0 - s[1]),
            1.0 - (1.0 - dst[2]) * (1.0 - s[2]),
        ],
        BlendMode::Multiply => [dst[0] * s[0], dst[1] * s[1], dst[2] * s[2]],
        BlendMode::Difference => [(dst[0] - s[0]).abs(), (dst[1] - s[1]).abs(), (dst[2] - s[2]).abs()],
        BlendMode::Lighten => [dst[0].max(s[0]), dst[1].max(s[1]), dst[2].max(s[2])],
        BlendMode::Darken => [dst[0].min(s[0]), dst[1].min(s[1]), dst[2].min(s[2])],
    };
    [lerp(dst[0], b[0], a), lerp(dst[1], b[1], a), lerp(dst[2], b[2], a)]
}

/// 2D layer transform in normalized canvas space (center origin, [-0.5,0.5]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub pos: [f32; 2],
    pub scale: [f32; 2],
    pub rotation: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Transform { pos: [0.0, 0.0], scale: [1.0, 1.0], rotation: 0.0 }
    }
}

impl Transform {
    pub fn clamp(&mut self) {
        for p in &mut self.pos {
            *p = p.clamp(-4.0, 4.0);
        }
        for s in &mut self.scale {
            *s = s.clamp(0.01, 16.0);
        }
        if !self.rotation.is_finite() {
            self.rotation = 0.0;
        }
    }
}

/// Shader attribution/licensing metadata (preserved across duplicate/reorder/
/// export). The app never auto-adds licensing claims to user shaders.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Attribution {
    pub title: String,
    pub author: String,
    pub source_url: String,
    pub license: String,
    pub license_url: String,
    pub modified_from: String,
    pub attribution_text: String,
}

/// A saved control-modulation mapping for a shader layer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModMapping {
    pub slot: u32,
    pub source: String,
    pub amount: f32,
    pub smoothing: f32,
}

/// Overlay (waveform/spectrum) layer configuration.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OverlayConfig {
    pub mode: u8,
    pub channel: u8,
    pub color: [f32; 4],
    pub scale: f32,
    pub thickness: f32,
    pub rotation: f32,
    pub points: f32,
    pub log_freq: bool,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        OverlayConfig {
            mode: 0,
            channel: 0,
            color: [0.2, 0.95, 0.6, 0.9],
            scale: 0.35,
            thickness: 0.006,
            rotation: 0.0,
            points: 128.0,
            log_freq: false,
        }
    }
}

// --- Multipass shader projects (Phase 8d) ---------------------------------

/// Max Buffer passes (A–D) plus one Image pass, and channels per pass.
pub const MAX_BUFFER_PASSES: usize = 4;
pub const MAX_PASSES: usize = MAX_BUFFER_PASSES + 1;
pub const MAX_CHANNELS: usize = 4;
/// Total source budget across all passes of one shader project.
pub const MAX_PROJECT_SOURCE: usize = 256 * 1024;

/// A `iChannelN` input source. Serialized as a lowercase string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelSource {
    None,
    Audio,
    /// Buffer A–D by index 0–3.
    Buffer(u8),
    /// This pass's own previous-frame output (feedback).
    SelfPrev,
}

impl ChannelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelSource::None => "none",
            ChannelSource::Audio => "audio",
            ChannelSource::Buffer(0) => "buffera",
            ChannelSource::Buffer(1) => "bufferb",
            ChannelSource::Buffer(2) => "bufferc",
            ChannelSource::Buffer(_) => "bufferd",
            ChannelSource::SelfPrev => "self",
        }
    }
    pub fn parse(s: &str) -> ChannelSource {
        match s {
            "audio" => ChannelSource::Audio,
            "buffera" => ChannelSource::Buffer(0),
            "bufferb" => ChannelSource::Buffer(1),
            "bufferc" => ChannelSource::Buffer(2),
            "bufferd" => ChannelSource::Buffer(3),
            "self" => ChannelSource::SelfPrev,
            _ => ChannelSource::None,
        }
    }
}

/// `pass_type` string ↔ execution index: Buffer A–D = 0–3, Image = 4.
pub fn pass_type_index(s: &str) -> Option<usize> {
    Some(match s {
        "buffera" => 0,
        "bufferb" => 1,
        "bufferc" => 2,
        "bufferd" => 3,
        "image" => 4,
        _ => return None,
    })
}
pub fn pass_type_str(index: usize) -> &'static str {
    match index {
        0 => "buffera",
        1 => "bufferb",
        2 => "bufferc",
        3 => "bufferd",
        _ => "image",
    }
}

fn default_channels() -> [String; 4] {
    std::array::from_fn(|_| "none".to_string())
}

/// One pass of a multipass shader project (a Buffer A–D or the Image pass).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PassData {
    pub pass_type: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub source: String,
    #[serde(default)]
    pub mode: u8,
    #[serde(default = "default_channels")]
    pub channels: [String; 4],
}

impl PassData {
    pub fn index(&self) -> Option<usize> {
        pass_type_index(&self.pass_type)
    }
}

/// Detect a dependency cycle among Buffer passes (excluding self-loops, which
/// are legitimate feedback). Returns true if any cycle exists. Cycles are not
/// rejected at runtime (they resolve via previous-frame history in fixed
/// A→B→C→D execution order); this is provided for diagnostics/tests.
pub fn buffer_graph_has_cycle(deps: &[[Option<u8>; 4]]) -> bool {
    let n = deps.len();
    // 0 = unvisited, 1 = in-progress, 2 = done
    let mut state = vec![0u8; n];
    fn dfs(v: usize, deps: &[[Option<u8>; 4]], state: &mut [u8]) -> bool {
        state[v] = 1;
        for ch in &deps[v] {
            if let Some(b) = ch {
                let b = *b as usize;
                if b == v || b >= deps.len() {
                    continue; // self-loop or out of range
                }
                if state[b] == 1 {
                    return true; // back-edge → cycle
                }
                if state[b] == 0 && dfs(b, deps, state) {
                    return true;
                }
            }
        }
        state[v] = 2;
        false
    }
    (0..n).any(|v| state[v] == 0 && dfs(v, deps, &mut state))
}

/// The effective pass list for a shader layer: an explicit multipass `passes`
/// list if present, otherwise a single Image pass migrated from the legacy
/// `source`/`mode` (with `iChannel0` = audio, matching prior behavior).
pub fn shader_project_passes(source: &str, mode: u8, passes: &[PassData]) -> Vec<PassData> {
    if !passes.is_empty() {
        return passes.to_vec();
    }
    let mut channels = default_channels();
    channels[0] = "audio".to_string();
    vec![PassData { pass_type: "image".to_string(), enabled: true, source: source.to_string(), mode, channels }]
}

/// Per-layer source and its serializable state.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SourceState {
    Milkdrop,
    Shader {
        source: String,
        mode: u8,
        controls: Vec<[f32; 4]>,
        #[serde(default)]
        mods: Vec<ModMapping>,
        #[serde(default)]
        attribution: Attribution,
        /// Multipass project (Buffer A–D + Image). Empty = legacy single-pass,
        /// where `source`/`mode` above is the sole Image pass.
        #[serde(default)]
        passes: Vec<PassData>,
    },
    Waveform(OverlayConfig),
    Spectrum(OverlayConfig),
}

impl SourceState {
    pub fn kind_str(&self) -> &'static str {
        match self {
            SourceState::Milkdrop => "milkdrop",
            SourceState::Shader { .. } => "shader",
            SourceState::Waveform(_) => "waveform",
            SourceState::Spectrum(_) => "spectrum",
        }
    }
}

/// Effect limits (documented; enforced by [`SceneData::validate`]).
pub const MAX_EFFECTS_PER_LAYER: usize = 8;
pub const MAX_GLOBAL_EFFECTS: usize = 8;
pub const MAX_TOTAL_EFFECTS: usize = 64;

/// A modulatable effect parameter's serialized state (mirrors `pm_params::Parameter`
/// config; min/max/meaning come from the effect type's Rust definition).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ParamData {
    pub base: f32,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub amount: f32,
    #[serde(default)]
    pub smoothing: f32,
    #[serde(default)]
    pub curve: String,
    #[serde(default)]
    pub invert: bool,
}

impl ParamData {
    pub fn new(base: f32) -> Self {
        ParamData { base, source: String::new(), amount: 0.0, smoothing: 0.0, curve: String::new(), invert: false }
    }
}

/// One effect in a chain. `effect_type` is a stable kind name; `params` are the
/// effect's parameters in a fixed, type-defined order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EffectData {
    pub id: u64,
    pub name: String,
    pub effect_type: String,
    pub enabled: bool,
    pub params: Vec<ParamData>,
}

fn default_true() -> bool {
    true
}

/// One layer's full serializable state.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LayerData {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub visible: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    #[serde(default)]
    pub transform: Transform,
    pub source: SourceState,
    #[serde(default)]
    pub effects: Vec<EffectData>,
}

/// A complete scene: ordered layers + global time/tempo settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SceneData {
    pub schema_version: u32,
    pub scene_id: String,
    pub name: String,
    pub layers: Vec<LayerData>,
    pub speed: f32,
    pub paused: bool,
    pub bpm: f32,
    pub tempo_manual: bool,
    pub subdivision: f32,
    #[serde(default)]
    pub global_effects: Vec<EffectData>,
}

impl SceneData {
    /// Validate + clamp in place. Errors on unknown schema or an oversize shader.
    pub fn validate(&mut self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported scene schema version {} (expected {SCHEMA_VERSION})",
                self.schema_version
            ));
        }
        if self.layers.len() > MAX_LAYERS {
            self.layers.truncate(MAX_LAYERS);
        }
        let mut shader_count = 0;
        let mut total_effects = 0;
        for l in &mut self.layers {
            l.opacity = l.opacity.clamp(0.0, 1.0);
            l.transform.clamp();
            if l.effects.len() > MAX_EFFECTS_PER_LAYER {
                l.effects.truncate(MAX_EFFECTS_PER_LAYER);
            }
            total_effects += l.effects.len();
            if let SourceState::Shader { source, passes, .. } = &mut l.source {
                shader_count += 1;
                if source.len() > MAX_SHADER_SOURCE {
                    return Err(format!("shader source exceeds {MAX_SHADER_SOURCE} bytes"));
                }
                // Multipass: bound pass count and per-pass + total source size.
                if passes.len() > MAX_PASSES {
                    passes.truncate(MAX_PASSES);
                }
                let mut total = source.len();
                for p in passes.iter() {
                    if p.source.len() > MAX_SHADER_SOURCE {
                        return Err(format!("shader pass source exceeds {MAX_SHADER_SOURCE} bytes"));
                    }
                    total += p.source.len();
                }
                if total > MAX_PROJECT_SOURCE {
                    return Err(format!("shader project source exceeds {MAX_PROJECT_SOURCE} bytes"));
                }
            }
        }
        if shader_count > MAX_SHADER_LAYERS {
            return Err(format!("too many shader layers ({shader_count} > {MAX_SHADER_LAYERS})"));
        }
        if self.global_effects.len() > MAX_GLOBAL_EFFECTS {
            self.global_effects.truncate(MAX_GLOBAL_EFFECTS);
        }
        total_effects += self.global_effects.len();
        if total_effects > MAX_TOTAL_EFFECTS {
            return Err(format!("too many effects ({total_effects} > {MAX_TOTAL_EFFECTS})"));
        }
        self.speed = self.speed.clamp(0.0, 8.0);
        self.bpm = self.bpm.clamp(20.0, 400.0);
        self.subdivision = self.subdivision.clamp(0.0625, 16.0);
        Ok(())
    }
}

/// Parse + validate a scene from JSON. Rejects oversize input up front, never
/// executes anything, and returns a clamped/validated scene or an error string.
pub fn parse_scene(json: &str) -> Result<SceneData, String> {
    if json.len() > MAX_SCENE_BYTES {
        return Err(format!("scene JSON exceeds {MAX_SCENE_BYTES} bytes"));
    }
    let mut scene: SceneData = serde_json::from_str(json).map_err(|e| format!("invalid scene JSON: {e}"))?;
    scene.validate()?;
    Ok(scene)
}

/// Serialize a scene to pretty JSON.
pub fn to_json(scene: &SceneData) -> String {
    serde_json::to_string_pretty(scene).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_scene() -> SceneData {
        SceneData {
            schema_version: SCHEMA_VERSION,
            scene_id: "s1".into(),
            name: "Test".into(),
            layers: vec![
                LayerData {
                    id: 1,
                    name: "Milkdrop".into(),
                    enabled: true,
                    visible: true,
                    opacity: 1.0,
                    blend: BlendMode::Normal,
                    transform: Transform::default(),
                    source: SourceState::Milkdrop,
                    effects: vec![EffectData {
                        id: 10,
                        name: "Bloom".into(),
                        effect_type: "bloom".into(),
                        enabled: true,
                        params: vec![ParamData::new(0.7), {
                            let mut p = ParamData::new(1.0);
                            p.source = "bass".into();
                            p.amount = 0.5;
                            p
                        }],
                    }],
                },
                LayerData {
                    id: 2,
                    name: "Shader".into(),
                    enabled: true,
                    visible: true,
                    opacity: 0.5,
                    blend: BlendMode::Add,
                    transform: Transform { pos: [0.1, -0.2], scale: [1.5, 1.5], rotation: 0.3 },
                    source: SourceState::Shader {
                        source: "void mainImage(out vec4 c, in vec2 f){c=vec4(1.0);}".into(),
                        mode: 0,
                        controls: vec![[0.5, 0.0, 0.0, 0.0]],
                        mods: vec![ModMapping { slot: 0, source: "bass".into(), amount: 0.5, smoothing: 0.2 }],
                        attribution: Attribution { author: "me".into(), license: "LGPL-2.1".into(), ..Default::default() },
                        passes: vec![],
                    },
                    effects: vec![],
                },
                LayerData {
                    id: 3,
                    name: "Waveform".into(),
                    enabled: true,
                    visible: true,
                    opacity: 0.8,
                    blend: BlendMode::Screen,
                    transform: Transform::default(),
                    source: SourceState::Waveform(OverlayConfig::default()),
                    effects: vec![],
                },
            ],
            speed: 1.0,
            paused: false,
            bpm: 120.0,
            tempo_manual: false,
            subdivision: 1.0,
            global_effects: vec![EffectData {
                id: 20,
                name: "Vignette".into(),
                effect_type: "vignette".into(),
                enabled: true,
                params: vec![ParamData::new(0.8), ParamData::new(0.5)],
            }],
        }
    }

    #[test]
    fn scene_round_trips() {
        let scene = sample_scene();
        let json = to_json(&scene);
        let back = parse_scene(&json).expect("round-trip");
        assert_eq!(scene, back);
    }

    #[test]
    fn attribution_survives_round_trip() {
        let json = to_json(&sample_scene());
        let back = parse_scene(&json).unwrap();
        if let SourceState::Shader { attribution, mods, source, .. } = &back.layers[1].source {
            assert_eq!(attribution.author, "me");
            assert_eq!(attribution.license, "LGPL-2.1");
            assert_eq!(mods[0].source, "bass");
            assert!(source.contains("mainImage"));
        } else {
            panic!("layer 1 should be a shader");
        }
    }

    #[test]
    fn unknown_schema_rejected() {
        let mut scene = sample_scene();
        scene.schema_version = 999;
        let json = serde_json::to_string(&scene).unwrap();
        assert!(parse_scene(&json).is_err());
    }

    #[test]
    fn opacity_and_transform_clamped() {
        let mut scene = sample_scene();
        scene.layers[0].opacity = 5.0;
        scene.layers[1].transform.scale = [1000.0, -1.0];
        scene.validate().unwrap();
        assert_eq!(scene.layers[0].opacity, 1.0);
        assert_eq!(scene.layers[1].transform.scale[0], 16.0);
        assert_eq!(scene.layers[1].transform.scale[1], 0.01);
    }

    #[test]
    fn layer_count_truncated() {
        let mut scene = sample_scene();
        let base = scene.layers[0].clone();
        scene.layers = (0..40).map(|i| LayerData { id: i, ..base.clone() }).collect();
        scene.validate().unwrap();
        assert_eq!(scene.layers.len(), MAX_LAYERS);
    }

    #[test]
    fn too_many_shaders_rejected() {
        let mut scene = sample_scene();
        let sh = scene.layers[1].clone();
        scene.layers = (0..MAX_SHADER_LAYERS + 1).map(|i| LayerData { id: i as u64, ..sh.clone() }).collect();
        assert!(scene.validate().is_err());
    }

    #[test]
    fn oversize_json_rejected() {
        let big = "x".repeat(MAX_SCENE_BYTES + 1);
        assert!(parse_scene(&big).is_err());
    }

    #[test]
    fn blend_modes_representative_values() {
        let dst = [0.4, 0.4, 0.4];
        let src = [0.6, 0.6, 0.6, 1.0];
        // Full opacity → blended color fully applied.
        assert_eq!(blend(BlendMode::Normal, dst, src, 1.0), [0.6, 0.6, 0.6]);
        assert_eq!(blend(BlendMode::Multiply, dst, src, 1.0)[0], 0.4 * 0.6);
        assert!((blend(BlendMode::Screen, dst, src, 1.0)[0] - (1.0 - 0.6 * 0.4)).abs() < 1e-6);
        assert!((blend(BlendMode::Difference, dst, src, 1.0)[0] - 0.2).abs() < 1e-6);
        assert_eq!(blend(BlendMode::Lighten, dst, src, 1.0)[0], 0.6);
        assert_eq!(blend(BlendMode::Darken, dst, src, 1.0)[0], 0.4);
        assert!((blend(BlendMode::Add, dst, src, 0.5)[0] - (0.4 + 0.6 * 0.5)).abs() < 1e-6);
        // Zero opacity → dst unchanged for every mode.
        for m in [BlendMode::Normal, BlendMode::Add, BlendMode::Screen, BlendMode::Multiply,
                  BlendMode::Difference, BlendMode::Lighten, BlendMode::Darken] {
            assert_eq!(blend(m, dst, src, 0.0), dst);
        }
    }

    #[test]
    fn blend_mode_u32_round_trips() {
        for m in [BlendMode::Normal, BlendMode::Add, BlendMode::Screen, BlendMode::Multiply,
                  BlendMode::Difference, BlendMode::Lighten, BlendMode::Darken] {
            assert_eq!(BlendMode::from_u32(m.as_u32()), m);
        }
    }

    #[test]
    fn legacy_single_pass_migrates_to_image() {
        // A shader layer with no `passes` migrates to one Image pass with audio.
        let passes = shader_project_passes("void mainImage(out vec4 c, in vec2 f){c=vec4(1.0);}", 0, &[]);
        assert_eq!(passes.len(), 1);
        assert_eq!(passes[0].pass_type, "image");
        assert_eq!(passes[0].channels[0], "audio");
    }

    #[test]
    fn multipass_round_trips() {
        let mut scene = sample_scene();
        scene.layers[1].source = SourceState::Shader {
            source: "void mainImage(out vec4 c, in vec2 f){c=texture(iChannel0, f/iResolution.xy);}".into(),
            mode: 0,
            controls: vec![],
            mods: vec![],
            attribution: Attribution::default(),
            passes: vec![
                PassData {
                    pass_type: "buffera".into(),
                    enabled: true,
                    source: "void mainImage(out vec4 c, in vec2 f){c=texture(iChannel0,f/iResolution.xy)*0.98+0.01;}".into(),
                    mode: 0,
                    channels: ["self".into(), "audio".into(), "none".into(), "none".into()],
                },
                PassData {
                    pass_type: "image".into(),
                    enabled: true,
                    source: "void mainImage(out vec4 c, in vec2 f){c=texture(iChannel0,f/iResolution.xy);}".into(),
                    mode: 0,
                    channels: ["buffera".into(), "none".into(), "none".into(), "none".into()],
                },
            ],
        };
        let back = parse_scene(&to_json(&scene)).unwrap();
        if let SourceState::Shader { passes, .. } = &back.layers[1].source {
            assert_eq!(passes.len(), 2);
            assert_eq!(passes[0].pass_type, "buffera");
            assert_eq!(passes[0].channels[0], "self");
            assert_eq!(passes[1].channels[0], "buffera");
        } else {
            panic!("expected shader");
        }
    }

    #[test]
    fn channel_source_round_trips() {
        for s in ["none", "audio", "buffera", "bufferb", "bufferc", "bufferd", "self"] {
            assert_eq!(ChannelSource::parse(s).as_str(), s);
        }
        assert_eq!(ChannelSource::parse("garbage"), ChannelSource::None);
        assert_eq!(pass_type_index("buffera"), Some(0));
        assert_eq!(pass_type_index("image"), Some(4));
        assert_eq!(pass_type_index("bogus"), None);
    }

    #[test]
    fn cycle_detection() {
        // A→B, B→Image (acyclic among buffers): no cycle. deps[i][ch] = Some(buffer_index).
        let acyclic = [
            [None, None, None, None],        // buffer A reads nothing
            [Some(0u8), None, None, None],   // buffer B reads A
        ];
        assert!(!buffer_graph_has_cycle(&acyclic));
        // A→B and B→A: cycle.
        let cyclic = [[Some(1u8), None, None, None], [Some(0u8), None, None, None]];
        assert!(buffer_graph_has_cycle(&cyclic));
        // Self-loop is feedback, not a cycle.
        let self_loop = [[Some(0u8), None, None, None]];
        assert!(!buffer_graph_has_cycle(&self_loop));
    }

    #[test]
    fn project_source_size_limits() {
        let mut scene = sample_scene();
        // One oversize pass is rejected.
        scene.layers[1].source = SourceState::Shader {
            source: "x".into(),
            mode: 0,
            controls: vec![],
            mods: vec![],
            attribution: Attribution::default(),
            passes: vec![PassData {
                pass_type: "image".into(),
                enabled: true,
                source: "x".repeat(MAX_SHADER_SOURCE + 1),
                mode: 0,
                channels: default_channels(),
            }],
        };
        assert!(scene.validate().is_err());
    }

    #[test]
    fn effects_survive_round_trip() {
        let back = parse_scene(&to_json(&sample_scene())).unwrap();
        assert_eq!(back.layers[0].effects[0].effect_type, "bloom");
        assert_eq!(back.layers[0].effects[0].params[1].source, "bass"); // modulation preserved
        assert!((back.layers[0].effects[0].params[1].amount - 0.5).abs() < 1e-6);
        assert_eq!(back.global_effects[0].effect_type, "vignette");
    }

    #[test]
    fn effects_per_layer_truncated() {
        let mut scene = sample_scene();
        let e = scene.layers[0].effects[0].clone();
        scene.layers[0].effects = (0..20).map(|i| EffectData { id: i, ..e.clone() }).collect();
        scene.validate().unwrap();
        assert_eq!(scene.layers[0].effects.len(), MAX_EFFECTS_PER_LAYER);
    }

    #[test]
    fn too_many_total_effects_rejected() {
        let mut scene = sample_scene();
        let e = scene.global_effects[0].clone();
        // Fill several layers to blow the total-effects budget.
        for l in &mut scene.layers {
            l.effects = (0..MAX_EFFECTS_PER_LAYER).map(|i| EffectData { id: 1000 + i as u64, ..e.clone() }).collect();
        }
        // 3 layers × 8 = 24, still under 64; add more layers of effects via clones.
        let full = scene.layers[0].clone();
        scene.layers = (0..MAX_LAYERS).map(|i| LayerData { id: i as u64, ..full.clone() }).collect();
        // MAX_LAYERS(16) × 8 = 128 > MAX_TOTAL_EFFECTS(64) → reject.
        assert!(scene.validate().is_err());
    }
}
