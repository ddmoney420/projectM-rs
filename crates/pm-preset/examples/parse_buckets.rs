//! Triage helper: for shaders that translate but fail naga *parsing*, bucket the
//! errors by a normalized key (the function/keyword the error names) so we know
//! which intrinsics/patterns dominate. Analysis only.
//!
//! ```text
//! cargo run -p pm-preset --example parse_buckets --release -- <dir> [<dir>...]
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

/// Reduce a naga parse error first-line to a stable bucket key.
fn bucket(msg: &str) -> Option<String> {
    let first = msg.lines().find(|l| l.contains("error:"))?;
    let first = first.trim();
    // Pull the back-ticked detail (function name / identifier) when present.
    let tick = |s: &str| -> Option<String> {
        let a = s.find('`')? + 1;
        let b = s[a..].find('`')? + a;
        Some(s[a..b].to_string())
    };
    if first.contains("inconsistent type passed as") {
        return Some(format!("inconsistent-arg -> {}", tick(first).unwrap_or_default()));
    }
    if first.contains("wrong type passed as") {
        return Some(format!("wrong-arg -> {}", tick(first).unwrap_or_default()));
    }
    if first.contains("is a reserved keyword") {
        return Some(format!("reserved-keyword -> {}", tick(first).unwrap_or_default()));
    }
    if first.contains("cannot cast") {
        return Some("cannot-cast".into());
    }
    if first.contains("automatic conversions cannot convert") {
        return Some("auto-convert (bool/int->vec?)".into());
    }
    if first.contains("invalid left-hand side") {
        return Some("invalid-lhs".into());
    }
    if first.contains("Unexpected runtime-expression") {
        return Some("runtime-expr global init".into());
    }
    None
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let mut files = Vec::new();
    for d in &dirs {
        collect(d, &mut files);
    }
    files.sort();

    let mut tally: HashMap<String, usize> = HashMap::new();
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
            if let Ok(t) = shader_to_wgsl(src, kind) {
                if let Err(e) = naga::front::wgsl::parse_str(&t.wgsl) {
                    if let Some(b) = bucket(&e.emit_to_string(&t.wgsl)) {
                        *tally.entry(b).or_default() += 1;
                    }
                }
            }
        }
    }

    let mut v: Vec<_> = tally.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    println!("Parse-error buckets (function/keyword named by the error):");
    for (k, n) in v.iter().take(30) {
        println!("  {n:>5}  {k}");
    }
}
