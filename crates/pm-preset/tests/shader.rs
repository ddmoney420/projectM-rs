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
