//! Triage helper (analysis only): for every shader that translates but is
//! naga-rejected, classify its *first* error bucket AND statically detect
//! indicators of the other long-tail buckets (D/E/F), so we can estimate how
//! many presets have more than one blocker (cascade potential).
//!
//! Indicators are heuristic textual scans of the generated WGSL / HLSL source:
//!   D  chained swizzle lvalue  (`_uv.xy.x`)         -> `.<2-4 swizzle>.<swizzle>`
//!   E  matrix construct/mul     (`uv * mat2x2(qb)`)  -> `mul(` / matrix ctor in body
//!   F  runtime global init      (`var<private> M = mat3x3(_qe…)`)
//! B (bool->float in constructors) can't be detected statically with confidence,
//! so it's only tracked via the first-error classification.
//!
//! ```text
//! cargo run -p pm-preset --example bucket_overlap --release -- <dir> [<dir>...]
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

/// First-error bucket (D/E/B/F/other-parse/validate) from naga.
fn first_bucket(wgsl: &str) -> &'static str {
    match naga::front::wgsl::parse_str(wgsl) {
        Err(e) => {
            let m = e.emit_to_string(wgsl);
            let line = m.lines().find(|l| l.contains("error:")).unwrap_or("");
            if line.contains("invalid left-hand side") {
                "D parse"
            } else if line.contains("cannot cast") {
                "E parse"
            } else if line.contains("Unexpected runtime-expression") {
                "F parse"
            } else if line.contains("wrong type passed as") || line.contains("automatic conversions") {
                "B parse"
            } else {
                "other parse"
            }
        }
        Ok(module) => {
            let mut v = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            );
            match v.validate(&module) {
                Ok(_) => "VALID",
                Err(_) => "validate",
            }
        }
    }
}

/// `_uv.xy.x = …` — a 2-4 char swizzle, another `.swizzle`, then an assignment
/// operator. Only the *lvalue* form is a blocker (rvalue chained swizzles are
/// valid WGSL), so require a trailing `=`/`+=`/… (but not `==`).
fn has_chained_swizzle_lvalue(wgsl: &str) -> bool {
    let sw = |c: char| "xyzwrgba".contains(c);
    let b: Vec<char> = wgsl.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '.' {
            let mut j = i + 1;
            while j < b.len() && sw(b[j]) {
                j += 1;
            }
            let len = j - (i + 1);
            // second swizzle: `.` + one or more swizzle chars
            if (2..=4).contains(&len) && j < b.len() && b[j] == '.' && j + 1 < b.len() && sw(b[j + 1]) {
                let mut k = j + 1;
                while k < b.len() && sw(b[k]) {
                    k += 1;
                }
                // skip spaces, then look for an assignment operator (not `==`).
                while k < b.len() && b[k] == ' ' {
                    k += 1;
                }
                if k < b.len() {
                    let op = b[k];
                    let next = b.get(k + 1).copied().unwrap_or(' ');
                    if (op == '=' && next != '=') || (matches!(op, '+' | '-' | '*' | '/') && next == '=') {
                        return true;
                    }
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    false
}

fn has_matrix_in_body(wgsl: &str) -> bool {
    wgsl.lines().any(|l| {
        !l.trim_start().starts_with("var<private>")
            && (l.contains("mat2x2<f32>(") || l.contains("mat3x3<f32>(") || l.contains("mat4x4<f32>(")
                || l.contains("mat3x4<f32>(") || l.contains("mat4x3<f32>("))
    })
}

/// A module-scope `var<private> X: … = …<identifier>` (initializer references a
/// runtime global), which WGSL rejects.
fn has_runtime_global_init(wgsl: &str) -> bool {
    wgsl.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("var<private>")
            && l.contains(" = ")
            && l.split(" = ").nth(1).is_some_and(|rhs| rhs.contains('_') || rhs.contains("rot") || rhs.contains("tex"))
    })
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let mut files = Vec::new();
    for d in &dirs {
        collect(d, &mut files);
    }
    files.sort();

    // bucket -> (count, also_D, also_E, also_F)
    let mut tally: HashMap<&'static str, [usize; 4]> = HashMap::new();
    let mut multi_blocker = 0usize; // rejected shaders with >=2 indicators present
    let mut rejected = 0usize;

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
            let bucket = first_bucket(&t.wgsl);
            if bucket == "VALID" {
                continue;
            }
            rejected += 1;

            let d = has_chained_swizzle_lvalue(&t.wgsl);
            let e = src.contains("mul(") || has_matrix_in_body(&t.wgsl);
            let f = has_runtime_global_init(&t.wgsl);
            let indicators = d as usize + e as usize + f as usize;
            if indicators >= 2 {
                multi_blocker += 1;
            }

            let entry = tally.entry(bucket).or_default();
            entry[0] += 1;
            entry[1] += d as usize;
            entry[2] += e as usize;
            entry[3] += f as usize;
        }
    }

    println!("Rejected shaders: {rejected}");
    println!(
        "With >=2 of the D/E/F indicators co-present: {multi_blocker} ({:.1}% of rejected)\n",
        100.0 * multi_blocker as f64 / rejected.max(1) as f64
    );
    println!("{:<14} {:>6}   also-D  also-E  also-F", "first-error", "count");
    let order = ["D parse", "E parse", "F parse", "B parse", "other parse", "validate"];
    for b in order {
        if let Some(t) = tally.get(b) {
            println!(
                "{b:<14} {:>6}   {:>5}   {:>5}   {:>5}",
                t[0], t[1], t[2], t[3]
            );
        }
    }
}
