//! Triage helper: print "kind|relpath" for every shader that produces VALID
//! WGSL, sorted — so two runs (before/after a change) can be diffed to find
//! regressions. Analysis only.

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
    let mut out = Vec::new();
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let content = String::from_utf8_lossy(&bytes);
        let Ok(preset) = Preset::load(&content) else { continue };
        let name = path.file_name().unwrap().to_string_lossy();
        for (src, kind, tag) in [
            (preset.warp_shader_source(), ShaderKind::Warp, "warp"),
            (preset.composite_shader_source(), ShaderKind::Composite, "comp"),
        ] {
            if !src.contains("shader_body") {
                continue;
            }
            if let Ok(t) = shader_to_wgsl(src, kind) {
                if valid(&t.wgsl) {
                    out.push(format!("{tag}|{name}"));
                }
            }
        }
    }
    out.sort();
    for l in out {
        println!("{l}");
    }
}
