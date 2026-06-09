//! Triage helper (analysis only): survey matrix usage across the corpus to scope
//! Bucket E and its visual risk.
//!   * how many shaders use `mul(` and/or a `floatNxM(...)` constructor,
//!   * split by whether the shader currently VALIDATES (so a `mul`-order flip
//!     would silently change its rendering) vs is REJECTED (the E target),
//!   * and the constructor shape (matrix-from-vec4 vs from-scalars).
//!
//! ```text
//! cargo run -p pm-preset --example matrix_survey --release -- <dir>...
//! ```

use pm_preset::{shader_to_wgsl, Preset, ShaderKind};
use std::path::{Path, PathBuf};

fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("milk")) {
            out.push(p);
        }
    }
}

fn valid(wgsl: &str) -> bool {
    let Ok(m) = naga::front::wgsl::parse_str(wgsl) else { return false };
    let mut v = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    v.validate(&m).is_ok()
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let mut files = Vec::new();
    for d in &dirs {
        collect(d, &mut files);
    }
    files.sort();

    let (mut uses_mul, mut uses_mat_ctor) = (0usize, 0usize);
    let (mut mul_valid, mut mul_rejected) = (0usize, 0usize);
    let (mut ctor_vec4, mut ctor_scalars) = (0usize, 0usize);
    let mut mul_and_mat = 0usize; // both mul( and a matrix constructor

    for path in &files {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let content = String::from_utf8_lossy(&bytes);
        let Ok(preset) = Preset::load(&content) else { continue };
        for (src, kind) in [
            (preset.warp_shader_source(), ShaderKind::Warp),
            (preset.composite_shader_source(), ShaderKind::Composite),
        ] {
            if !src.contains("shader_body") {
                continue;
            }
            let Ok(t) = shader_to_wgsl(src, kind) else { continue };

            let has_mul = src.contains("mul(");
            let has_ctor = src.contains("float2x2") || src.contains("float3x3") || src.contains("float4x4")
                || src.contains("float3x4") || src.contains("float4x3");
            if !has_mul && !has_ctor {
                continue;
            }
            let is_valid = valid(&t.wgsl);

            if has_mul {
                uses_mul += 1;
                if is_valid {
                    mul_valid += 1;
                } else {
                    mul_rejected += 1;
                }
            }
            if has_ctor {
                uses_mat_ctor += 1;
                // matrix-from-vec4 if a constructor wraps a single identifier
                // (heuristic: `float2x2(_q` / `float2x2(q` with no comma before `)`)
                let from_vec4 = t.wgsl.contains("mat2x2<f32>(_") || t.wgsl.contains("mat3x3<f32>(_")
                    || t.wgsl.contains("mat2x2<f32>(q") || t.wgsl.contains("mat4x4<f32>(_");
                if from_vec4 {
                    ctor_vec4 += 1;
                } else {
                    ctor_scalars += 1;
                }
            }
            if has_mul && has_ctor {
                mul_and_mat += 1;
            }
        }
    }

    println!("Shaders using mul(:           {uses_mul}");
    println!("  currently VALID (visual risk if mul flips): {mul_valid}");
    println!("  currently REJECTED (Bucket E target):       {mul_rejected}");
    println!("Shaders using a matrix constructor: {uses_mat_ctor}");
    println!("  from-vec4 indicator: {ctor_vec4}");
    println!("  from-scalars/other:  {ctor_scalars}");
    println!("Shaders using BOTH mul( and a matrix ctor: {mul_and_mat}");
}
