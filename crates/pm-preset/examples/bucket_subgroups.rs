//! Triage helper (analysis only): subgroup the remaining shader buckets.
//!  * InvalidUnaryOperandType by op, and whether the operand looks like a bool
//!    (`-(a < b)`), i.e. a bool→numeric cascade.
//!  * InvalidImageCoordinateType by texture dimension (D2/D3), and whether a
//!    `tex2D` coordinate is wider than vec2 (e.g. `GetBlur1(uv)` -> vec3).
//!  * cannot-cast (Bucket E) by matrix pattern: matrix-from-vec4 vs from-scalars,
//!    `mul(` vs operator `*`.
//!
//! ```text
//! cargo run -p pm-preset --example bucket_subgroups --release -- <dir>...
//! ```

use pm_preset::{shader_to_wgsl, Preset, ShaderKind};
use std::collections::HashMap;
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

fn bump(m: &mut HashMap<String, usize>, k: impl Into<String>) {
    *m.entry(k.into()).or_default() += 1;
}

/// Does the WGSL contain a unary minus directly on a comparison? `-((… < …))`
fn has_neg_on_compare(wgsl: &str) -> bool {
    // crude: `-(` followed (within a small window) by a comparison operator
    // before the matching close — good enough for triage.
    let b = wgsl.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'-' && b[i + 1] == b'(' {
            let win = &wgsl[i..(i + 80).min(wgsl.len())];
            // a comparison, but not `<f32>` / `<i32>` generics or `->`
            if win.contains(" < ") || win.contains(" > ") || win.contains(" <= ") || win.contains(" >= ")
                || win.contains(" == ") || win.contains(" != ")
            {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let mut files = Vec::new();
    for d in &dirs {
        collect(d, &mut files);
    }
    files.sort();

    let mut unary: HashMap<String, usize> = HashMap::new();
    let mut unary_neg_bool = 0usize;
    let mut image: HashMap<String, usize> = HashMap::new();
    let mut image_tex2d_wide = 0usize;
    let mut ecast: HashMap<String, usize> = HashMap::new();
    let (mut ucount, mut icount, mut ecount) = (0, 0, 0);

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

            // Parse-stage cannot-cast = Bucket E.
            match naga::front::wgsl::parse_str(&t.wgsl) {
                Err(e) => {
                    let msg = e.emit_to_string(&t.wgsl);
                    if msg.lines().any(|l| l.contains("cannot cast")) {
                        ecount += 1;
                        // matrix-from-vec4 (single non-numeric ctor arg) vs scalars
                        let from_vec4 = t.wgsl.contains("mat2x2<f32>(_") || t.wgsl.contains("mat3x3<f32>(_")
                            || t.wgsl.contains("mat2x2<f32>(q") || t.wgsl.contains("mat4x4<f32>(_");
                        if src.contains("mul(") {
                            bump(&mut ecast, "uses mul(");
                        }
                        if from_vec4 {
                            bump(&mut ecast, "matrix-from-vec4 ctor");
                        } else {
                            bump(&mut ecast, "matrix-from-scalars / other");
                        }
                    }
                    continue;
                }
                Ok(module) => {
                    let mut v = naga::valid::Validator::new(
                        naga::valid::ValidationFlags::all(),
                        naga::valid::Capabilities::all(),
                    );
                    if let Err(e) = v.validate(&module) {
                        let dbg = format!("{e:?}");
                        if let Some(i) = dbg.find("InvalidUnaryOperandType(") {
                            ucount += 1;
                            let op = dbg[i + 24..].split([',', ')']).next().unwrap_or("?").to_string();
                            bump(&mut unary, op);
                            if has_neg_on_compare(&t.wgsl) {
                                unary_neg_bool += 1;
                            }
                        } else if let Some(i) = dbg.find("InvalidImageCoordinateType(") {
                            icount += 1;
                            let dim = dbg[i + 27..].split([',', ')']).next().unwrap_or("?").to_string();
                            bump(&mut image, dim);
                            // a tex2D coordinate that is wider than vec2 (GetBlur etc.)
                            if t.wgsl.contains("sampler_main, sampler_main_sampler, (textureSample")
                                || t.wgsl.contains(".xyz * vec3<f32>(_c5")
                            {
                                image_tex2d_wide += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    let show = |title: &str, m: &HashMap<String, usize>| {
        println!("== {title} ==");
        let mut v: Vec<_> = m.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        for (k, n) in v {
            println!("  {n:>5}  {k}");
        }
    };
    println!("InvalidUnary total: {ucount}  (with neg-on-compare pattern: {unary_neg_bool})");
    show("InvalidUnary by op", &unary);
    println!("\nInvalidImageCoord total: {icount}  (tex2D wide-coord indicator: {image_tex2d_wide})");
    show("InvalidImageCoord by texture dim", &image);
    println!("\ncannot-cast (E) total: {ecount}");
    show("cannot-cast indicators (non-exclusive)", &ecast);
}
