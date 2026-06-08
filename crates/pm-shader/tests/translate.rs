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

#[test]
fn vector_truncation_on_assignment() {
    // HLSL implicitly truncates a wider vector to the target width: a vec4
    // assigned to a float3, and a vec3 compound-assigned to a scalar `.x`.
    let src = r#"
        void PS(float2 _uv : TEXCOORD0, out float4 _return_value : COLOR) {
            float3 a = float4(0.1, 0.2, 0.3, 0.4);
            float3 b = float3(1.0, 2.0, 3.0);
            float s = float3(0.5, 0.6, 0.7);
            a.x += b;
            _return_value = float4(a, s);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    // float4 -> float3 keeps .xyz; vec3 -> scalar keeps .x.
    assert!(wgsl.contains(".xyz"), "vec4 truncated to vec3");
    assert!(wgsl.contains(").x"), "vec3 truncated to scalar on +=");
}

#[test]
fn const_qualifier_and_float1_and_scalar_swizzle() {
    // `const`/`static` qualifiers, the 1-component `float1` scalar type, and
    // HLSL scalar swizzles (`s.x` -> s, `s.xxx` -> broadcast).
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            const float a = 0.5;
            static float1 b = 0.25;
            float c = a.x + b.x;
            float3 v = c.xxx;
            _return_value = float4(v, c);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("vec3<f32>(c)"), "scalar .xxx broadcast");
}

#[test]
fn comma_operator_statement() {
    // A top-level comma operator becomes a sequence of statements.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float3 ret = float3(0.0, 0.0, 0.0);
            ret += float3(0.1, 0.0, 0.0),
            ret += float3(0.0, 0.2, 0.0),
            ret = ret * 2.0;
            _return_value = float4(ret, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
}

#[test]
fn binary_op_truncates_wider_vector() {
    // HLSL truncates the wider operand of a binary op to the narrower width:
    // `tan(vec4) * vec2` operates on the first two lanes -> vec2.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float4 big = float4(1.0, 2.0, 3.0, 4.0);
            float2 small = float2(0.5, 0.25);
            float2 rs = big * small;
            _return_value = float4(rs, rs);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains(").xy"), "wider operand truncated to vec2");
}

#[test]
fn lerp_broadcasts_to_widest_and_float_increment() {
    // `lerp(scalar, scalar, vec3)` must broadcast the scalars to vec3 so `mix`
    // gets consistent operands; HLSL `n++` on a float lowers to `n = n + 1.0`.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float3 a = float3(1.0, 2.0, 3.0);
            float n = 0.0;
            n++;
            float3 m = lerp(0.0, 1.0, a);
            _return_value = float4(m, n);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("n = (n + 1.0)") || wgsl.contains("n = n + 1.0"), "float ++ lowered to add");
}

#[test]
fn user_function_signature_inference_and_arg_coercion() {
    // A user-defined helper's return type drives inference of its call, and
    // arguments coerce to the declared parameter types (HLSL broadcasts a
    // scalar `0.5` to a `float2` parameter).
    let src = r#"
        float2 polar(float2 domain, float2 center) {
            return domain - center;
        }
        void PS(float2 _uv : TEXCOORD0, out float4 _return_value : COLOR) {
            float2 p = polar(_uv, 0.5) * float2(0.5, 1.0);
            _return_value = float4(p, 0.0, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    // The scalar arg is broadcast to the vec2 parameter.
    assert!(wgsl.contains("polar(_uv, vec2<f32>(0.5))"), "scalar arg broadcast to vec2 param");
}
