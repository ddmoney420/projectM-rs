//! Parser tests over realistic Milkdrop HLSL fragments.

use pm_shader::*;

fn parse_ok(src: &str) -> Vec<Item> {
    parse(src).unwrap_or_else(|e| panic!("parse failed: {e}\nsource:\n{src}"))
}

#[test]
fn parse_global_uniforms_comma_list() {
    let items = parse_ok("uniform float4 _c1, _c2, _c3, _c4;");
    assert_eq!(items.len(), 4);
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Global { uniform, ty, name, .. } => {
                assert!(uniform);
                assert_eq!(*ty, Type::Float4);
                assert_eq!(name, &format!("_c{}", i + 1));
            }
            _ => panic!("expected global"),
        }
    }
}

#[test]
fn parse_sampler_decl() {
    let items = parse_ok("sampler2D sampler_main;");
    assert!(matches!(&items[0], Item::Sampler { ty: Type::Sampler2D, name } if name == "sampler_main"));
}

#[test]
fn parse_ps_warp_signature() {
    let src = r#"
        void PS(float4 _vDiffuse : COLOR,
                float4 _uv : TEXCOORD0,
                float2 _rad_ang : TEXCOORD1,
                out float4 _return_value : COLOR0,
                out float4 _mv_tex_coords : COLOR1)
        {
            float3 ret = 0;
        }
    "#;
    let items = parse_ok(src);
    let Item::Function(f) = &items[0] else { panic!("expected function") };
    assert_eq!(f.name, "PS");
    assert_eq!(f.params.len(), 5);
    assert_eq!(f.params[0].semantic.as_deref(), Some("COLOR"));
    assert_eq!(f.params[3].qualifier, ParamQual::Out);
    assert_eq!(f.params[3].name, "_return_value");
}

#[test]
fn parse_expression_constructs_and_swizzles() {
    let src = "void PS() { float3 c = float3(1.0, 0.5, 0.0); ret.xy = c.zy * 2.0; }";
    let items = parse_ok(src);
    let Item::Function(f) = &items[0] else { panic!() };
    // Decl with a constructor initializer.
    assert!(matches!(&f.body[0], Stmt::Decl { ty: Type::Float3, .. }));
    // Swizzle assignment.
    assert!(matches!(&f.body[1], Stmt::Expr(Expr::Assign(..))));
}

#[test]
fn parse_control_flow() {
    let src = r#"
        void PS() {
            float x = 0;
            for (int i = 0; i < 8; i++) {
                x += 1.0;
            }
            if (x > 4.0) { x = 4.0; } else { x = 0.0; }
        }
    "#;
    let items = parse_ok(src);
    let Item::Function(f) = &items[0] else { panic!() };
    assert!(matches!(f.body[1], Stmt::For(..)));
    assert!(matches!(f.body[2], Stmt::If(..)));
}

#[test]
fn parse_precedence_and_ternary() {
    // 1 + 2 * 3 must group the multiply; ternary must parse.
    let src = "void PS() { float a = 1.0 + 2.0 * 3.0; float b = a > 0.0 ? 1.0 : -1.0; }";
    let items = parse_ok(src);
    let Item::Function(f) = &items[0] else { panic!() };
    let Stmt::Decl { init: Some(Expr::Binary(BinOp::Add, _, rhs)), .. } = &f.body[0] else {
        panic!("expected add at top of precedence tree")
    };
    assert!(matches!(**rhs, Expr::Binary(BinOp::Mul, _, _)));
    assert!(matches!(&f.body[1], Stmt::Decl { init: Some(Expr::Ternary(..)), .. }));
}

#[test]
fn parse_cast_expression() {
    let src = "void PS() { float3 a = (float3)1.0; }";
    let items = parse_ok(src);
    let Item::Function(f) = &items[0] else { panic!() };
    assert!(matches!(&f.body[0], Stmt::Decl { init: Some(Expr::Construct(Type::Float3, _)), .. }));
}

#[test]
fn parse_full_preprocessed_warp_body() {
    // The shape produced by the preprocessor: header defines expanded, PS entry.
    let src = r#"
        #define time _c2.x
        #define GetPixel(uv) (tex2D(sampler_main,uv).xyz)
        uniform float4 _c2;
        sampler2D sampler_main;
        void PS(float2 _uv : TEXCOORD0, out float4 _return_value : COLOR) {
            float3 ret = 0;
            ret = GetPixel(_uv) * (0.5 + 0.5 * sin(time));
            _return_value = float4(ret.xyz, 1.0);
        }
    "#;
    let pp = preprocess(src);
    let items = parse(&pp).unwrap_or_else(|e| panic!("parse failed: {e}\npreprocessed:\n{pp}"));
    // uniform + sampler + function.
    assert!(items.iter().any(|i| matches!(i, Item::Function(f) if f.name == "PS")));
    assert!(items.iter().any(|i| matches!(i, Item::Sampler { .. })));
}
