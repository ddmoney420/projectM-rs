//! Diagnostic: across the corpus, tally which identifiers naga reports as
//! "no definition in scope" so we know which Milkdrop built-ins to declare.
//!
//! ```text
//! cargo run -p pm-preset --example undef_idents --release -- <dir> [<dir>...]
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

fn undef_ident(wgsl: &str) -> Option<String> {
    let module = naga::front::wgsl::parse_str(wgsl);
    if let Err(e) = module {
        let msg = e.emit_to_string(wgsl);
        let line = msg.lines().find(|l| l.contains("no definition in scope for identifier"))?;
        // ...identifier: `name`
        let start = line.find('`')? + 1;
        let end = line[start..].find('`')? + start;
        return Some(line[start..end].to_string());
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
                if let Some(id) = undef_ident(&t.wgsl) {
                    *tally.entry(id).or_default() += 1;
                }
            }
        }
    }

    let mut v: Vec<_> = tally.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    println!("Top undefined identifiers (first one per failing shader):");
    for (id, n) in v.iter().take(40) {
        println!("  {n:>6}  {id}");
    }
}
