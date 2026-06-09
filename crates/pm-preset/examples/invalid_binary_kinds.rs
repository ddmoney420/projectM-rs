//! Triage helper (analysis only): for shaders that parse but fail naga
//! validation with `InvalidBinaryOperandTypes`, parse the `op` and operand
//! types out of the error debug and subgroup by operand *shape* (matrix mul,
//! vector-width mismatch, int/float, bool, scalar/vector, …).
//!
//! ```text
//! cargo run -p pm-preset --example invalid_binary_kinds --release -- <dir>...
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

/// Extract `field: ` value up to the next top-level `,` or `}` (best-effort).
fn field<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let i = s.find(key)? + key.len();
    let rest = &s[i..];
    // stop at the next `, <ident>:` or ` }` — good enough for these shapes.
    let mut depth = 0i32;
    for (j, c) in rest.char_indices() {
        match c {
            '{' | '(' => depth += 1,
            '}' | ')' => depth -= 1,
            ',' if depth == 0 => return Some(rest[..j].trim()),
            _ => {}
        }
        if depth < 0 {
            return Some(rest[..j].trim());
        }
    }
    Some(rest.trim())
}

/// Classify a naga type debug string into a short shape tag.
fn shape(ty: &str) -> &'static str {
    if ty.contains("Matrix") {
        "mat"
    } else if ty.contains("Vector") {
        let k = if ty.contains("kind: Bool") { 'b' } else if ty.contains("kind: Float") { 'f' } else { 'i' };
        match (ty.contains("size: Bi"), ty.contains("size: Tri"), ty.contains("size: Quad")) {
            (true, _, _) => if k == 'b' { "vec2b" } else if k == 'f' { "vec2f" } else { "vec2i" },
            (_, true, _) => if k == 'b' { "vec3b" } else if k == 'f' { "vec3f" } else { "vec3i" },
            (_, _, true) => if k == 'b' { "vec4b" } else if k == 'f' { "vec4f" } else { "vec4i" },
            _ => "vec?",
        }
    } else if ty.contains("kind: Bool") {
        "boolS"
    } else if ty.contains("kind: Float") {
        "floatS"
    } else {
        "intS" // Sint / Uint
    }
}

/// Higher-level category for the (op, lhs, rhs) triple.
fn category(op: &str, l: &str, r: &str) -> &'static str {
    let is_mul = op == "Multiply";
    if is_mul && (l == "mat" || r == "mat") {
        return "matrix * vector / vector * matrix";
    }
    if l == "mat" || r == "mat" {
        return "matrix in non-multiply op";
    }
    let lv = l.starts_with("vec");
    let rv = r.starts_with("vec");
    if lv && rv {
        // both vectors
        if l != r {
            return "vector width / kind mismatch";
        }
        return "same-width vectors (other)";
    }
    if lv != rv {
        return "scalar / vector mismatch";
    }
    // both scalar
    let lb = l == "boolS";
    let rb = r == "boolS";
    if lb || rb {
        return "bool / numeric scalar";
    }
    if (l == "floatS" && r == "intS") || (l == "intS" && r == "floatS") {
        return "int / float scalar";
    }
    "other"
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let mut files = Vec::new();
    for d in &dirs {
        collect(d, &mut files);
    }
    files.sort();

    let mut cat: HashMap<&'static str, usize> = HashMap::new();
    let mut triple: HashMap<String, (usize, String)> = HashMap::new();
    let mut total = 0;

    for path in &files {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let content = String::from_utf8_lossy(&bytes);
        let Ok(preset) = Preset::load(&content) else { continue };
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for (src, kind) in [
            (preset.warp_shader_source(), ShaderKind::Warp),
            (preset.composite_shader_source(), ShaderKind::Composite),
        ] {
            if !src.contains("shader_body") {
                continue;
            }
            let Ok(t) = shader_to_wgsl(src, kind) else { continue };
            let Ok(module) = naga::front::wgsl::parse_str(&t.wgsl) else { continue };
            let mut v = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            );
            if let Err(e) = v.validate(&module) {
                let dbg = format!("{e:?}");
                if !dbg.contains("InvalidBinaryOperandTypes") {
                    continue;
                }
                total += 1;
                let op = field(&dbg, "op: ").unwrap_or("?");
                let l = shape(field(&dbg, "lhs_type: ").unwrap_or(""));
                let r = shape(field(&dbg, "rhs_type: ").unwrap_or(""));
                *cat.entry(category(op, l, r)).or_default() += 1;
                let key = format!("{op:<10} {l:>6} , {r:<6}");
                let entry = triple.entry(key).or_insert((0, name.clone()));
                entry.0 += 1;
            }
        }
    }

    println!("InvalidBinaryOperandTypes total: {total}\n");
    println!("== by category ==");
    let mut cv: Vec<_> = cat.into_iter().collect();
    cv.sort_by(|a, b| b.1.cmp(&a.1));
    for (c, n) in cv {
        println!("  {n:>5}  {c}");
    }
    println!("\n== by (op, lhs, rhs) ==");
    let mut tv: Vec<_> = triple.into_iter().collect();
    tv.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for (k, (n, sample)) in tv.into_iter().take(16) {
        println!("  {n:>5}  {k}   e.g. {sample}");
    }
}
