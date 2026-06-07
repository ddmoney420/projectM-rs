//! End-to-end HLSL → WGSL translation tests, with the generated WGSL validated
//! by naga (the same compiler wgpu uses) to prove it's well-formed.

use pm_shader::translate;

/// Parse + validate WGSL with naga. Returns Ok(()) or a descriptive error.
fn validate_wgsl(wgsl: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| {
        format!("naga parse error: {}\n--- WGSL ---\n{wgsl}", e.emit_to_string(wgsl))
    })?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    validator
        .validate(&module)
        .map_err(|e| format!("naga validation error: {e:?}\n--- WGSL ---\n{wgsl}"))?;
    Ok(())
}

fn translate_ok(src: &str) -> String {
    translate(src).unwrap_or_else(|e| panic!("translation failed: {e}\nsource:\n{src}"))
}

#[test]
fn translates_and_validates_warp_body() {
    // A realistic preprocessed warp shader (no textures, so it's self-contained).
    let src = r#"
        #define time _c2.x
        #define bass _c3.x
        uniform float4 _c2;
        uniform float4 _c3;
        void PS(float4 _vDiffuse : COLOR,
                float4 _uv : TEXCOORD0,
                float2 _rad_ang : TEXCOORD1,
                out float4 _return_value : COLOR0,
                out float4 _mv_tex_coords : COLOR1)
        {
            float3 ret = 0;
            _mv_tex_coords.xy = _uv.xy;
            float2 uv = _uv.xy;
            float t = time + bass;
            ret = float3(0.5 + 0.5 * sin(t), uv.x, uv.y);
            ret *= 1.0 - length(uv - 0.5);
            _return_value = float4(ret.xyz, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    // Spot-check key transforms.
    assert!(wgsl.contains("struct PSOutput"), "out params -> struct");
    assert!(wgsl.contains("vec3<f32>(0.0)"), "scalar->vector promotion of `ret = 0`");
    assert!(wgsl.contains("fract") || wgsl.contains("sin("), "intrinsic emitted");
}

#[test]
fn ternary_becomes_select() {
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float3 ret = 0;
            float x = 0.5;
            ret = x > 0.0 ? float3(1.0, 0.0, 0.0) : float3(0.0, 0.0, 1.0);
            _return_value = float4(ret, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("select("), "ternary lowered to select()");
}

#[test]
fn intrinsics_are_remapped_and_valid() {
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float3 a = float3(0.2, 0.4, 0.6);
            float3 b = lerp(a, float3(1.0, 1.0, 1.0), 0.5);
            float3 c = saturate(b * 2.0);
            float3 d = frac(c) + float3(rsqrt(2.0), 0.0, 0.0);
            _return_value = float4(d, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("mix("), "lerp -> mix");
    assert!(wgsl.contains("clamp("), "saturate -> clamp");
    assert!(wgsl.contains("fract("), "frac -> fract");
    assert!(wgsl.contains("inverseSqrt("), "rsqrt -> inverseSqrt");
}

#[test]
fn for_loop_and_scalar_promotion_validate() {
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float3 acc = 0;
            for (int i = 0; i < 8; i++) {
                acc += float3(0.1, 0.1, 0.1);
            }
            _return_value = float4(acc, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("while ("), "for lowered to while");
}

#[test]
fn swizzle_lvalue_expands() {
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float4 v = float4(0.0, 0.0, 0.0, 1.0);
            float2 src = float2(0.3, 0.7);
            v.xy = src;
            _return_value = v;
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    // Expanded to component-wise writes via a temp.
    assert!(wgsl.contains(".x =") && wgsl.contains(".y ="), "multi-swizzle lvalue expanded");
}

#[test]
fn texture_sampling_uses_binding_convention() {
    // Texture calls won't validate standalone (need real bindings), so just
    // check the textureSample emission + binding-convention comment.
    let src = r#"
        sampler2D sampler_main;
        void PS(float2 _uv : TEXCOORD0, out float4 _return_value : COLOR) {
            float3 ret = tex2D(sampler_main, _uv).xyz;
            _return_value = float4(ret, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    assert!(wgsl.contains("textureSample(sampler_main, sampler_main_sampler, _uv)"));
    assert!(wgsl.contains("sampler_main_sampler"));
}
