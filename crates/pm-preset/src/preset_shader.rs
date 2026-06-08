//! Milkdrop preset shader → render-ready WGSL.
//!
//! Ports `MilkdropShader::PreprocessPresetShader`: wraps the preset's
//! `shader_body { … }` into a `PS(…)` entry function, prepends the uniform +
//! intrinsic header, and transpiles it to WGSL via [`pm_shader`]. It then
//! assembles a complete module — uniform buffer, texture/sampler bindings, a
//! fullscreen vertex stage and a fragment entry that runs `PS` — so the output
//! is directly usable as a wgpu shader.
//!
//! Uniform *values* (the `_c0.._c13`, `_q*`, rotation matrices) are filled by
//! the renderer per frame; this module only declares and wires them.

use pm_shader::ParseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderKind {
    Warp,
    Composite,
}

/// A translated preset shader: the WGSL module plus the texture/sampler names
/// it binds, in binding order (`textures[i]` is at `@binding(2*i+1)`, its
/// sampler at `@binding(2*i+2)`).
#[derive(Debug, Clone)]
pub struct TranslatedShader {
    pub wgsl: String,
    pub textures: Vec<String>,
}

/// Translate a preset shader body to a complete, render-ready WGSL module.
pub fn to_wgsl(body: &str, kind: ShaderKind) -> Result<TranslatedShader, ShaderError> {
    let wrapped = wrap(body, kind)?;
    let translated = pm_shader::translate(&wrapped)?;
    let (wgsl, textures) = assemble(&translated, kind);
    Ok(TranslatedShader { wgsl, textures })
}

/// The translated warp shader's core, for the renderer to assemble into a
/// mesh-vertex pipeline (the warp fragment runs over the warp mesh, not
/// fullscreen, so the renderer supplies the vertex stage and bindings).
pub struct WarpShaderParts {
    /// `var<private>` uniform globals + the `PS` function (no bindings/entry).
    pub ps_wgsl: String,
    /// Textures the shader samples, in first-reference order.
    pub textures: Vec<String>,
}

/// Translate a preset *warp* shader to its PS core (for mesh rendering).
pub fn warp_shader_parts(body: &str) -> Result<WarpShaderParts, ShaderError> {
    let wrapped = wrap(body, ShaderKind::Warp)?;
    let translated = pm_shader::translate(&wrapped)?;
    let textures = find_textures(&translated);
    Ok(WarpShaderParts { ps_wgsl: translated, textures })
}

/// The `MdUniforms` WGSL struct definition (no `@group`/`@binding`).
pub fn md_uniforms_struct() -> String {
    let mut s = String::from("struct MdUniforms {\n");
    for (_, field, ty) in uniforms() {
        s.push_str(&format!("    {field}: {ty},\n"));
    }
    s.push_str("}\n");
    s
}

/// The `load_uniforms()` WGSL that copies the `md` block into the private
/// globals the PS code reads.
pub fn md_load_uniforms() -> String {
    let mut s = String::from("fn load_uniforms() {\n");
    for (var, field, _) in uniforms() {
        s.push_str(&format!("    {var} = md.{field};\n"));
    }
    s.push_str("}\n");
    s
}

/// The fixed list of Milkdrop shader uniforms: `(private var, struct field,
/// wgsl type)`. The same layout is uploaded by the renderer every frame.
fn uniforms() -> Vec<(String, String, &'static str)> {
    let mut v = Vec::new();
    for n in 0..=13 {
        v.push((format!("_c{n}"), format!("c{n}"), "vec4<f32>"));
    }
    for l in 'a'..='h' {
        v.push((format!("_q{l}"), format!("q{l}"), "vec4<f32>"));
    }
    v.push(("rand_frame".into(), "rand_frame".into(), "vec4<f32>"));
    v.push(("rand_preset".into(), "rand_preset".into(), "vec4<f32>"));
    for group in ["s", "d", "f", "vf", "uf", "rand"] {
        for i in 1..=4 {
            let name = format!("rot_{group}{i}");
            v.push((name.clone(), name, "mat3x4<f32>"));
        }
    }
    v
}

/// Assemble the final WGSL: uniform block + texture bindings + the translated
/// globals/`PS` function + a `load_uniforms` copy + a fullscreen entry.
fn assemble(translated: &str, kind: ShaderKind) -> (String, Vec<String>) {
    let uniforms = uniforms();
    let mut out = String::new();

    // 1. Uniform buffer.
    out.push_str("struct MdUniforms {\n");
    for (_, field, ty) in &uniforms {
        out.push_str(&format!("    {field}: {ty},\n"));
    }
    out.push_str("}\n@group(0) @binding(0) var<uniform> md: MdUniforms;\n\n");

    // 2. Texture/sampler bindings for the samplers the shader references.
    let textures = find_textures(translated);
    let mut binding = 1u32;
    for tex in &textures {
        out.push_str(&format!("@group(0) @binding({binding}) var {tex}: texture_2d<f32>;\n"));
        binding += 1;
        out.push_str(&format!("@group(0) @binding({binding}) var {tex}_sampler: sampler;\n"));
        binding += 1;
    }
    out.push('\n');

    // 3. The transpiled globals (var<private> uniforms) and PS function.
    out.push_str(translated);
    out.push('\n');

    // 4. Copy the uniform block into the private globals the PS code reads.
    out.push_str("fn load_uniforms() {\n");
    for (var, field, _) in &uniforms {
        out.push_str(&format!("    {var} = md.{field};\n"));
    }
    out.push_str("}\n\n");

    // 5. Fullscreen vertex stage + fragment entry running PS.
    out.push_str(ENTRY_PRELUDE);
    let ps_call = match kind {
        ShaderKind::Warp => "PS(diffuse, vec4<f32>(in.uv, in.uv), ra)",
        ShaderKind::Composite => "PS(diffuse, in.uv, ra)",
    };
    out.push_str(&format!(
        "@fragment\nfn fs_main(in: VsOut) -> @location(0) vec4<f32> {{\n    load_uniforms();\n    let ra = rad_ang_of(in.uv);\n    let diffuse = vec4<f32>(1.0, 1.0, 1.0, 1.0);\n    let o = {ps_call};\n    return vec4<f32>(o._return_value.rgb, 1.0);\n}}\n"
    ));

    (out, textures)
}

/// Find the textures referenced as `textureSample(<name>, …)`.
fn find_textures(wgsl: &str) -> Vec<String> {
    let mut found = Vec::new();
    let needle = "textureSample(";
    let mut rest = wgsl;
    while let Some(pos) = rest.find(needle) {
        let after = &rest[pos + needle.len()..];
        let name: String = after.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if !name.is_empty() && !found.contains(&name) {
            found.push(name);
        }
        rest = after;
    }
    found
}

const ENTRY_PRELUDE: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0),
    );
    let p = corners[vid];
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, p.y * 0.5 + 0.5);
    return out;
}

// Polar coordinates the preset reads as `rad`/`ang` (aspect = _c0).
fn rad_ang_of(uv: vec2<f32>) -> vec2<f32> {
    let px = (uv.x * 2.0 - 1.0) * _c0.x;
    let py = (uv.y * 2.0 - 1.0) * _c0.y;
    let denom = sqrt(_c0.x * _c0.x + _c0.y * _c0.y);
    var rad = 0.0;
    if (denom > 0.0) { rad = sqrt(px * px + py * py) / denom; }
    var ang = atan2(py, px);
    if (ang < 0.0) { ang = ang + 6.2831853; }
    return vec2<f32>(rad, ang);
}
"#;

/// The Milkdrop preset-shader header: uniform declarations (so the transpiler
/// infers `vec4` types for `_cN.x` swizzles) and the `#define` intrinsics
/// preset authors use. Trimmed to the functional parts of `PresetShaderHeader`.
const HEADER: &str = r#"
#define M_PI 3.14159265359
#define M_PI_2 6.28318530718
#define M_INV_PI_2 0.159154943091895
uniform float4 rand_frame;
uniform float4 rand_preset;
uniform float4 _c0;
uniform float4 _c1, _c2, _c3, _c4, _c5, _c6, _c7, _c8, _c9, _c10, _c11, _c12, _c13;
uniform float4 _qa, _qb, _qc, _qd, _qe, _qf, _qg, _qh;
uniform float4x3 rot_s1, rot_s2, rot_s3, rot_s4;
uniform float4x3 rot_d1, rot_d2, rot_d3, rot_d4;
uniform float4x3 rot_f1, rot_f2, rot_f3, rot_f4;
uniform float4x3 rot_vf1, rot_vf2, rot_vf3, rot_vf4;
uniform float4x3 rot_uf1, rot_uf2, rot_uf3, rot_uf4;
uniform float4x3 rot_rand1, rot_rand2, rot_rand3, rot_rand4;

#define time     _c2.x
#define fps      _c2.y
#define frame    _c2.z
#define progress _c2.w
#define bass _c3.x
#define mid  _c3.y
#define treb _c3.z
#define vol  _c3.w
#define bass_att _c4.x
#define mid_att  _c4.y
#define treb_att _c4.z
#define vol_att  _c4.w
#define q1 _qa.x
#define q2 _qa.y
#define q3 _qa.z
#define q4 _qa.w
#define q5 _qb.x
#define q6 _qb.y
#define q7 _qb.z
#define q8 _qb.w
#define q9 _qc.x
#define q10 _qc.y
#define q11 _qc.z
#define q12 _qc.w
#define q13 _qd.x
#define q14 _qd.y
#define q15 _qd.z
#define q16 _qd.w
#define q17 _qe.x
#define q18 _qe.y
#define q19 _qe.z
#define q20 _qe.w
#define q21 _qf.x
#define q22 _qf.y
#define q23 _qf.z
#define q24 _qf.w
#define q25 _qg.x
#define q26 _qg.y
#define q27 _qg.z
#define q28 _qg.w
#define q29 _qh.x
#define q30 _qh.y
#define q31 _qh.z
#define q32 _qh.w
#define aspect   _c0
#define texsize  _c7
#define roam_cos _c8
#define roam_sin _c9
#define slow_roam_cos _c10
#define slow_roam_sin _c11
#define mip_x   _c12.x
#define mip_y   _c12.y
#define mip_avg _c12.z
#define blur1_min _c6.z
#define blur1_max _c6.w
#define blur2_min _c13.x
#define blur2_max _c13.y
#define blur3_min _c13.z
#define blur3_max _c13.w
#define GetMain(uv) (tex2D(sampler_main,uv).xyz)
#define GetPixel(uv) (tex2D(sampler_main,uv).xyz)
#define GetBlur1(uv) (tex2D(sampler_blur1,uv).xyz*_c5.x + _c5.y)
#define GetBlur2(uv) (tex2D(sampler_blur2,uv).xyz*_c5.z + _c5.w)
#define GetBlur3(uv) (tex2D(sampler_blur3,uv).xyz*_c6.x + _c6.y)
#define lum(x) (dot(x,float3(0.32,0.49,0.29)))
#define tex2d tex2D
#define tex3d tex3D
"#;

/// Wrap a preset's raw shader body into a full HLSL translation unit:
/// `shader_body` → a `PS()` function, with the header and per-shader defines
/// prepended. Mirrors `PreprocessPresetShader`.
pub fn wrap(body: &str, kind: ShaderKind) -> Result<String, ShaderError> {
    let body = strip_sampler_state(body);

    let entry_pos = body.find("shader_body").ok_or(ShaderError::MissingEntry)?;

    let signature = match kind {
        ShaderKind::Warp => {
            "\nvoid PS(float4 _vDiffuse : COLOR,\n        float4 _uv : TEXCOORD0,\n        float2 _rad_ang : TEXCOORD1,\n        out float4 _return_value : COLOR0,\n        out float4 _mv_tex_coords : COLOR1)\n"
        }
        ShaderKind::Composite => {
            "\nvoid PS(float4 _vDiffuse : COLOR,\n        float2 _uv : TEXCOORD0,\n        float2 _rad_ang : TEXCOORD1,\n        out float4 _return_value : COLOR)\n"
        }
    };

    // Replace "shader_body" with the entry signature.
    let mut program = String::new();
    program.push_str(&body[..entry_pos]);
    program.push_str(signature);
    let after_entry = &body[entry_pos + "shader_body".len()..];

    // Replace the opening brace with the variable declarations.
    let brace = after_entry.find('{').ok_or(ShaderError::MissingBrace)?;
    program.push_str(&after_entry[..brace]);
    program.push_str("{\nfloat3 ret = 0;\n");
    if kind == ShaderKind::Warp {
        program.push_str("_mv_tex_coords.xy = _uv.xy;\n");
    }
    let after_brace = &after_entry[brace + 1..];

    // Replace the final closing brace with the return assignment.
    let last_brace = after_brace.rfind('}').ok_or(ShaderError::MissingBrace)?;
    program.push_str(&after_brace[..last_brace]);
    program.push_str("\n_return_value = float4(ret.xyz, 1.0);\n}\n");

    // Prepend the header and the per-shader coordinate defines.
    let mut full = String::new();
    full.push_str(HEADER);
    match kind {
        ShaderKind::Warp => full.push_str(
            "#define rad _rad_ang.x\n#define ang _rad_ang.y\n#define uv _uv.xy\n#define uv_orig _uv.zw\n",
        ),
        ShaderKind::Composite => full.push_str(
            "#define rad _rad_ang.x\n#define ang _rad_ang.y\n#define uv _uv.xy\n#define uv_orig _uv.xy\n#define hue_shader _vDiffuse.xyz\n",
        ),
    }
    full.push_str(&program);
    Ok(full)
}

/// Remove `sampler_state { … };` override blocks, which have no WGSL/GLSL
/// equivalent (`PreprocessPresetShader` does the same).
fn strip_sampler_state(src: &str) -> String {
    let mut out = src.to_string();
    while let Some(pos) = out.find("sampler_state") {
        // Back up to the preceding '=' and forward to the closing '};'.
        let start = out[..pos].rfind('=').unwrap_or(pos);
        let after = &out[pos..];
        if let Some(brace) = after.find('}') {
            if let Some(semi) = after[brace..].find(';') {
                let end = pos + brace + semi + 1;
                out.replace_range(start..end, "");
                continue;
            }
        }
        break;
    }
    out
}

#[derive(Debug)]
pub enum ShaderError {
    MissingEntry,
    MissingBrace,
    Translate(ParseError),
}

impl std::fmt::Display for ShaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShaderError::MissingEntry => write!(f, "preset shader has no 'shader_body' entry point"),
            ShaderError::MissingBrace => write!(f, "preset shader is missing braces"),
            ShaderError::Translate(e) => write!(f, "HLSL->WGSL translation failed: {e}"),
        }
    }
}

impl std::error::Error for ShaderError {}

impl From<ParseError> for ShaderError {
    fn from(e: ParseError) -> Self {
        ShaderError::Translate(e)
    }
}
