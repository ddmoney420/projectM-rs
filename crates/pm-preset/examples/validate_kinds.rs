//! Diagnostic: across the corpus, classify naga *validation* failures by their
//! error variant, so we know which validate bug to fix next.
//!
//! ```text
//! cargo run -p pm-preset --example validate_kinds --release -- <dir> [<dir>...]
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

/// A short tag for the validation error variant.
fn classify(wgsl: &str) -> Option<(String, String)> {
    let module = naga::front::wgsl::parse_str(wgsl).ok()?;
    let mut v = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    let err = v.validate(&module).err()?;
    let dbg = format!("{err:?}");
    // Pull out the inner variant name (e.g. InvalidImageCoordinateType, ...).
    let tag = ["InvalidImageCoordinateType", "InvalidStoreTypes", "InvalidArgumentType",
               "WrongArgumentCount", "InvalidImageStore", "NotPointer", "InvalidStore",
               "InvalidSwizzle", "TypeResolution", "Compose", "Call", "EmitResult"]
        .iter()
        .find(|t| dbg.contains(**t))
        .map(|t| t.to_string())
        .unwrap_or_else(|| {
            // fall back to first 40 chars after "source:"
            dbg.find("source:")
                .map(|i| dbg[i + 7..].chars().take(40).collect::<String>())
                .unwrap_or_else(|| "other".into())
        });
    Some((tag, dbg.chars().take(110).collect()))
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let mut files = Vec::new();
    for d in &dirs {
        collect(d, &mut files);
    }
    files.sort();

    let mut tally: HashMap<String, usize> = HashMap::new();
    let mut samples: HashMap<String, String> = HashMap::new();
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
                if let Some((tag, full)) = classify(&t.wgsl) {
                    *tally.entry(tag.clone()).or_default() += 1;
                    samples.entry(tag).or_insert(full);
                }
            }
        }
    }

    let mut v: Vec<_> = tally.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    println!("Validation-failure variants:");
    for (tag, n) in v.iter().take(20) {
        println!("  {n:>6}  {tag}");
        if let Some(s) = samples.get(tag) {
            println!("            {s}");
        }
    }
}
