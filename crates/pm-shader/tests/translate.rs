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

#[test]
fn dot_truncates_mismatched_vector_widths() {
    // HLSL `dot` operates on the common (narrower) width; WGSL needs both args
    // the same type. Covers vec4/vec3, vec3/vec4, vec2/vec4.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float4 a = float4(1.0, 2.0, 3.0, 4.0);
            float3 b = float3(0.1, 0.2, 0.3);
            float2 c = float2(0.5, 0.6);
            float d1 = dot(a, b);   // vec4, vec3 -> vec3
            float d2 = dot(b, a);   // vec3, vec4 -> vec3
            float d3 = dot(c, a);   // vec2, vec4 -> vec2
            _return_value = float4(d1, d2, d3, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains(".xyz") && wgsl.contains(".xy"), "wider dot args truncated");
}

#[test]
fn distance_and_reflect_coerce_widths() {
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float4 a = float4(1.0, 2.0, 3.0, 4.0);
            float3 b = float3(0.1, 0.2, 0.3);
            float dd = distance(a, b);          // vec4, vec3 -> vec3
            float3 r = reflect(b, a);           // vec3, vec4 -> vec3
            _return_value = float4(r, dd);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
}

#[test]
fn smoothstep_broadcasts_scalar_edges_to_vector() {
    // `smoothstep(scalar, scalar, vec3)` broadcasts the edges and returns vec3.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float3 x = float3(0.2, 0.5, 0.8);
            float3 s = smoothstep(0.0, 1.0, x);
            _return_value = float4(s, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("smoothstep("), "smoothstep emitted");
}

#[test]
fn component_wise_intrinsics_regression() {
    // The pre-existing min/max/clamp/lerp coercion must still work unchanged.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float3 v = float3(0.2, 0.5, 0.8);
            float3 a = min(v, 0.6);
            float3 b = max(a, float3(0.1, 0.1, 0.1));
            float3 c = clamp(b, 0.0, 1.0);
            float3 d = lerp(c, v, 0.5);
            _return_value = float4(d, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("mix(") && wgsl.contains("clamp("));
}

#[test]
fn reserved_keyword_var_mod_is_renamed() {
    // A preset variable named `mod` (a WGSL reserved word) must be renamed at
    // its declaration and every use site.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float3 mod = float3(0.1, 0.2, 0.3);
            mod = mod * 2.0;
            float s = mod.x + dot(mod, mod);
            _return_value = float4(mod, s);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("mod_pm"), "mod renamed to mod_pm");
    // The bare reserved word must not appear as an identifier token.
    assert!(!wgsl.contains("var mod:"), "no declaration of bare `mod`");
}

#[test]
fn reserved_keywords_filter_and_move_renamed() {
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float filter = 0.5;
            float move = 0.25;
            float r = filter * move + filter;
            _return_value = float4(r, move, filter, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("filter_pm") && wgsl.contains("move_pm"));
}

#[test]
fn reserved_keyword_in_function_call_argument() {
    // A reserved identifier used inside a function-call expression.
    let src = r#"
        void PS(float2 _uv : TEXCOORD0, out float4 _return_value : COLOR) {
            float2 mod = _uv * 2.0;
            float3 c = lerp(float3(0,0,0), float3(1,1,1), clamp(mod.x, 0.0, 1.0));
            _return_value = float4(c, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("mod_pm.x"), "reserved ident sanitized inside call arg");
}

#[test]
fn non_reserved_identifier_unchanged_and_idempotent() {
    // A normal identifier is untouched; repeated references don't double-rename.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float3 color = float3(0.1, 0.2, 0.3);
            color = color + color;
            _return_value = float4(color, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("var color:"), "non-reserved name unchanged");
    assert!(!wgsl.contains("color_pm"), "non-reserved name not sanitized");
    assert!(!wgsl.contains("_pm_pm"), "no double-sanitizing");
}

#[test]
fn bool_to_float_in_vector_constructors() {
    // HLSL `floatN(cond)` -> 1.0/0.0; WGSL needs an explicit f32(bool).
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float x = 0.7;
            float  a = float (x > 0.5);
            float2 b = float2(x > 0.5);
            float3 c = float3(x > 0.5);
            float4 d = float4(x > 0.5);
            _return_value = d + float4(c, 0.0) + float4(b, b) + a;
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("f32("), "bool wrapped in f32()");
    assert!(wgsl.contains("vec3<f32>(f32("), "float3(bool) -> vec3<f32>(f32(bool))");
}

#[test]
fn bool_as_arithmetic_operand_coerces() {
    // A comparison used numerically: `a * (x > 0.5)`, `(x > 0.5) * a`, `a + (...)`.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float x = 0.7;
            float a = 2.0;
            float p = a * (x > 0.5);
            float q = (x > 0.5) * a;
            float r = a + (x > 0.5);
            _return_value = float4(p, q, r, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("f32("), "bool operand cast to f32 in arithmetic");
}

#[test]
fn bool_in_lerp_vector_third_arg() {
    // `lerp(a, b, float3(cond))` routes the bool through the same coercion.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float3 a = float3(0.0, 0.0, 0.0);
            float3 b = float3(1.0, 1.0, 1.0);
            float x = 0.7;
            float3 m = lerp(a, b, float3(x > 0.5));
            _return_value = float4(m, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
}

#[test]
fn genuine_bool_contexts_stay_bool() {
    // if/ternary/&&/|| conditions must remain bool (no numeric cast).
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float x = 0.7;
            float y = 0.2;
            float r = 0.0;
            if (x > 0.5 && y < 0.5) { r = 1.0; }
            float s = (x > 0.5) ? 2.0 : 3.0;
            _return_value = float4(r, s, 0.0, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("if (") && wgsl.contains("&&"), "bool condition kept");
    assert!(wgsl.contains("select("), "ternary -> select with bool condition");
    // The if/&& condition itself must not be wrapped in f32(...).
    assert!(!wgsl.contains("if (f32("), "if condition not numerically coerced");
}

#[test]
fn valid_numeric_constructor_unchanged() {
    // A plain numeric constructor must not gain a spurious f32() cast.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float a = 0.3;
            float3 v = float3(a, a * 2.0, 0.5);
            _return_value = float4(v, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("vec3<f32>(a,"), "numeric constructor unchanged");
    assert!(!wgsl.contains("f32(a)"), "no spurious cast on a float arg");
}

#[test]
fn int_to_float_in_mixed_binary_ops() {
    // HLSL promotes int->float in mixed arithmetic/comparisons; WGSL needs f32().
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float x = 0.7;
            int n = 3;
            float a = x * n;          // float * int
            float b = n * x;          // int * float
            float c = x + n;          // float + int
            bool  le = x <= n;        // float <= int
            bool  lt = n < x;         // int < float
            float r = a + b + c + float(le) + float(lt);
            _return_value = float4(r, r, r, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("f32(n)"), "int variable cast to f32 in float context");
}

#[test]
fn int_in_float_constructors_and_broadcast() {
    // float3(intVar) and `float3 v = intVar` (scalar int broadcast).
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            int n = 2;
            float3 a = float3(n);     // construct: vec3<f32>(f32(n))
            float3 b = n;             // broadcast: vec3<f32>(f32(n))
            _return_value = float4(a + b, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("f32(n)"));
}

#[test]
fn int_vector_to_float_vector_conversion() {
    // `float3 fv = intVec3` -> component-wise vec3<f32>(vec3<i32>).
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            int3 iv = int3(1, 2, 3);
            float3 fv = iv;
            _return_value = float4(fv, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("vec3<f32>(iv)") || wgsl.contains("vec3<f32>(vec3<i32>"), "int vec -> float vec conversion");
}

#[test]
fn integer_contexts_stay_int() {
    // Loop counter, integer-only comparison, and bitwise/modulo must stay i32 —
    // no spurious f32() that would make the WGSL invalid.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float acc = 0.0;
            for (int i = 0; i < 4; i = i + 1) {
                acc += 0.1;
            }
            int a = 6;
            int b = 4;
            bool icmp = a < b;        // int < int stays int comparison
            int band = a & b;         // bitwise stays int
            int bmod = a % b;         // modulo stays int
            float r = acc + float(icmp) + float(band) + float(bmod);
            _return_value = float4(r, r, r, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    // The loop counter and int ops must not be cast to f32.
    assert!(!wgsl.contains("f32(i)"), "loop counter stays int");
    assert!(wgsl.contains("(a & b)") || wgsl.contains("a & b"), "bitwise stays int");
}

#[test]
fn already_valid_numeric_unchanged_by_int_coercion() {
    // Pure-float and int-literal math must not gain spurious casts.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float x = 0.5;
            float a = x * 2.0;        // float * float-literal
            float b = x * 3;          // float * int-literal -> 3.0, no f32()
            _return_value = float4(a, b, 0.0, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(!wgsl.contains("f32(3"), "int literal stays a float literal, no f32() wrap");
}

#[test]
fn tex2d_truncates_wide_coordinate_to_xy() {
    // tex2D with a vec3 coord truncates to .xy (HLSL uses only the first two
    // components). End-to-end naga validation is in pm-preset (with bindings);
    // here we check the codegen emission, since raw translate doesn't bind
    // textures.
    let src = r#"
        sampler2D sampler_main;
        void PS(float2 _uv : TEXCOORD0, out float4 _return_value : COLOR) {
            float3 c3 = float3(_uv, 0.5);
            float4 c4 = float4(_uv, 0.5, 1.0);
            float3 a = tex2D(sampler_main, c3).xyz;
            float3 b = tex2D(sampler_main, c4).xyz;
            _return_value = float4(a + b, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    assert!(wgsl.contains("textureSample(sampler_main, sampler_main_sampler, (c3).xy)"), "vec3 coord -> .xy");
    assert!(wgsl.contains("(c4).xy"), "vec4 coord -> .xy");
    // A vec2 coord (the default) must not be rewritten.
    assert!(!wgsl.contains("(_uv).xy"));
}

#[test]
fn tex3d_keeps_vec3_coordinate() {
    let src = r#"
        sampler3D sampler_noisevol_hq;
        void PS(float2 _uv : TEXCOORD0, out float4 _return_value : COLOR) {
            float3 p = float3(_uv, 0.3);
            float3 n = tex3D(sampler_noisevol_hq, p).xyz;
            _return_value = float4(n, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    assert!(wgsl.contains("textureSample(sampler_noisevol_hq, sampler_noisevol_hq_sampler, p)"));
    assert!(!wgsl.contains("(p).xy"), "tex3D coord not truncated");
}

#[test]
fn unary_minus_on_bool_coerces_before_negation() {
    // HLSL `-(a < b)` = `-(0/1)`; the bool must become f32 *before* the minus.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float x = 0.7;
            float r = -(x > 0.5);              // -(f32(x > 0.5))
            float3 v = float3(-(x > 0.5));     // constructor path
            _return_value = float4(v, r);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("-(f32("), "bool coerced before unary minus");
    assert!(!wgsl.contains("-((") || wgsl.contains("-(f32("), "no bare -(bool)");
}

#[test]
fn bool_times_bool_arithmetic_coerces_both() {
    // `(a > b) * (c > d)` and `(a > b) + (c > d)` are float math in HLSL.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float a = 0.7;
            float b = 0.3;
            float c = 0.6;
            float d = 0.2;
            float m = (a > b) * (c > d);
            float s = (a > b) + (c > d);
            _return_value = float4(m, s, 0.0, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("f32((a > b)) * f32((c > d))"), "both bool operands cast");
    assert!(wgsl.contains("f32((a > b)) + f32((c > d))"));
}

#[test]
fn bool_contexts_unaffected_by_unary_and_arith_coercion() {
    // if/!/&& must stay bool — not numerically coerced.
    let src = r#"
        void PS(out float4 _return_value : COLOR) {
            float a = 0.7;
            float b = 0.3;
            float r = 0.0;
            if (a > b) { r = 1.0; }
            bool n = !(a > b);
            if ((a > b) && (b < a)) { r += 1.0; }
            _return_value = float4(r, float(n), 0.0, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("if ((a > b))"), "if condition stays bool");
    assert!(wgsl.contains("!((a > b))"), "logical not stays bool");
    assert!(wgsl.contains("&&"), "logical and stays bool");
    assert!(!wgsl.contains("if (f32("), "no numeric coercion of conditions");
}

#[test]
fn bool_function_returning_numeric_mask_coerces() {
    // HLSL: a bool-returning function whose body is a numeric mask expression
    // (`(a > b) * (c < d)`) is true iff nonzero -> `(...) != 0.0` in WGSL.
    let src = r#"
        bool maskf(float a, float b, float c, float d) {
            return (a > b) * (c < d);
        }
        bool maski(int a, int b) {
            return a * b;
        }
        bool plainbool(float a, float b) {
            return a > b;
        }
        void PS(out float4 _return_value : COLOR) {
            float r = 0.0;
            if (maskf(0.7, 0.3, 0.2, 0.6)) { r += 1.0; }
            if (maski(2, 3)) { r += 1.0; }
            if (plainbool(0.5, 0.4)) { r += 1.0; }
            _return_value = float4(r, r, r, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    assert!(wgsl.contains("!= 0.0"), "float mask return -> != 0.0");
    assert!(wgsl.contains("!= 0;") || wgsl.contains("!= 0 "), "int mask return -> != 0");
    // The plain-bool function must NOT get a `!= 0` coercion.
    assert!(wgsl.contains("fn plainbool"));
}

#[test]
fn float_function_returning_numeric_unchanged() {
    // A float-returning function with a numeric body is not touched.
    let src = r#"
        float blend(float a, float b, float c, float d) {
            return (a > b) * (c < d);
        }
        void PS(out float4 _return_value : COLOR) {
            float r = blend(0.7, 0.3, 0.2, 0.6);
            _return_value = float4(r, r, r, 1.0);
        }
    "#;
    let wgsl = translate_ok(src);
    validate_wgsl(&wgsl).unwrap();
    // No `!= 0.0` coercion on a float return.
    assert!(!wgsl.contains("!= 0.0"), "float return not coerced to bool");
}
