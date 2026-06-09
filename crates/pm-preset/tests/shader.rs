//! Preset-shader → WGSL tests, validated with naga (wgpu's compiler).

use pm_preset::{shader_to_wgsl, ShaderKind};

fn validate(wgsl: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(wgsl)
        .map_err(|e| format!("naga parse error: {}\n--- WGSL ---\n{wgsl}", e.emit_to_string(wgsl)))?;
    let mut v = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    v.validate(&module).map_err(|e| format!("naga validation error: {e:?}\n--- WGSL ---\n{wgsl}"))?;
    Ok(())
}

#[test]
fn default_composite_shader_compiles() {
    // The built-in default composite shader.
    let body = "shader_body\n{\nret = tex2D(sampler_main, uv).xyz;\n}";
    let wgsl = shader_to_wgsl(body, ShaderKind::Composite).expect("translate").wgsl;
    validate(&wgsl).unwrap();
    assert!(wgsl.contains("textureSample(sampler_main"));
    assert!(wgsl.contains("@fragment"));
}

#[test]
fn composite_with_uniforms_and_intrinsics() {
    // Uses time (_c2.x), bass (_c3.x), lum(), GetPixel(), and ret math.
    let body = r#"shader_body
{
    ret = GetPixel(uv);
    ret *= 1.0 + 0.3 * bass;
    ret.r = ret.r * (0.5 + 0.5 * sin(time));
    float l = lum(ret);
    ret = lerp(ret, float3(l, l, l), 0.2);
}"#;
    let wgsl = shader_to_wgsl(body, ShaderKind::Composite).expect("translate").wgsl;
    validate(&wgsl).unwrap();
    assert!(wgsl.contains("md.c2")); // time uniform wired
}

#[test]
fn warp_shader_compiles() {
    let body = r#"shader_body
{
    ret = tex2D(sampler_main, uv).xyz;
    ret *= 0.99;
    ret += GetPixel(uv + float2(0.001, 0.0)) * 0.1;
}"#;
    let wgsl = shader_to_wgsl(body, ShaderKind::Warp).expect("translate").wgsl;
    validate(&wgsl).unwrap();
    // Warp has a second output for motion vectors.
    assert!(wgsl.contains("_mv_tex_coords"));
}

#[test]
fn getblur_references_blur_samplers() {
    let body = "shader_body { ret = GetBlur1(uv) * 0.5 + GetPixel(uv) * 0.5; }";
    let wgsl = shader_to_wgsl(body, ShaderKind::Composite).expect("translate").wgsl;
    validate(&wgsl).unwrap();
    assert!(wgsl.contains("sampler_blur1"));
}

#[test]
fn missing_shader_body_errors() {
    assert!(shader_to_wgsl("ret = uv.x;", ShaderKind::Composite).is_err());
}

#[test]
fn warp_uv_lvalue_write_compiles() {
    // The Bucket D pattern: `uv.x += d` (and `uv.y -= d`) must not produce an
    // invalid chained-swizzle lvalue (`_uv.xy.x`). `uv` is now a mutable local.
    let body = r#"shader_body
{
    float d = 0.01;
    uv.x += d;
    uv.y -= d;
    ret = tex2D(sampler_main, uv).xyz;
}"#;
    let wgsl = shader_to_wgsl(body, ShaderKind::Warp).expect("translate").wgsl;
    validate(&wgsl).unwrap();
    assert!(wgsl.contains("var uv: vec2<f32> = _uv.xy"), "uv is a mutable local");
    assert!(!wgsl.contains("_uv.xy.x +="), "no chained-swizzle lvalue");
}

#[test]
fn warp_uv_full_swizzle_assignment_compiles() {
    // `uv.xy = some_vec2` (whole-vector reassignment).
    let body = r#"shader_body
{
    float2 off = float2(0.1, 0.2);
    uv.xy = uv + off;
    ret = tex2D(sampler_main, uv).xyz;
}"#;
    let wgsl = shader_to_wgsl(body, ShaderKind::Warp).expect("translate").wgsl;
    validate(&wgsl).unwrap();
}

#[test]
fn warp_uv_orig_read_path_compiles() {
    // `uv_orig` (warp = `_uv.zw`) read path still works.
    let body = r#"shader_body
{
    ret = tex2D(sampler_main, uv).xyz;
    ret += tex2D(sampler_main, uv_orig).xyz * 0.2;
}"#;
    let wgsl = shader_to_wgsl(body, ShaderKind::Warp).expect("translate").wgsl;
    validate(&wgsl).unwrap();
    assert!(wgsl.contains("var uv_orig: vec2<f32> = _uv.zw"), "uv_orig seeded from _uv.zw");
}

#[test]
fn read_only_uv_still_compiles_unchanged() {
    // Shaders that only READ uv must still work (and now read the local).
    let body = "shader_body\n{\nret = tex2D(sampler_main, uv).xyz * 0.98;\n}";
    let warp = shader_to_wgsl(body, ShaderKind::Warp).expect("translate").wgsl;
    let comp = shader_to_wgsl(body, ShaderKind::Composite).expect("translate").wgsl;
    validate(&warp).unwrap();
    validate(&comp).unwrap();
    // The sample now goes through the `uv` local, not an inlined `_uv.xy`.
    assert!(warp.contains("textureSample(sampler_main, sampler_main_sampler, uv)"));
}

#[test]
fn non_uv_swizzle_assignment_unchanged() {
    // A normal (non-uv) single-level swizzle lvalue still works as before.
    let body = r#"shader_body
{
    ret = tex2D(sampler_main, uv).xyz;
    ret.x = 0.5;
    ret.yz = float2(0.2, 0.3);
}"#;
    let wgsl = shader_to_wgsl(body, ShaderKind::Composite).expect("translate").wgsl;
    validate(&wgsl).unwrap();
}
