//! Triage helper (analysis only): group translate-stage (HLSL parser) failures
//! by their `(unexpected token, expected construct)` signature — the parser's
//! own description of which grammar rule broke — with per-kind counts and a few
//! example presets each. For the `Ident` buckets it also tallies the specific
//! identifier names.
//!
//! ```text
//! cargo run -p pm-preset --example parser_drill --release -- <dir>...
//! ```

use pm_preset::{shader_to_wgsl, Preset, ShaderError, ShaderKind};
use pm_shader::{ParseError, Tok};
use std::collections::BTreeMap;
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

/// Short name for the unexpected token (collapsing `Ident("x")` to `Ident`).
fn tok_kind(t: &Tok) -> String {
    match t {
        Tok::Ident(_) => "Ident".into(),
        other => format!("{other:?}").split(['(', ' ']).next().unwrap_or("?").to_string(),
    }
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let mut files = Vec::new();
    for d in &dirs {
        collect(d, &mut files);
    }
    files.sort();

    // key = "WARP/COMP  <tok> | expected <what>"
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut idents: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;

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
            let err = match shader_to_wgsl(src, kind) {
                Ok(_) => continue,
                Err(e) => e,
            };
            let ShaderError::Translate(pe) = err else { continue };
            let ParseError::Unexpected { got, expected } = pe else { continue };
            total += 1;
            let k = match kind {
                ShaderKind::Warp => "WARP",
                ShaderKind::Composite => "COMP",
            };
            if let Tok::Ident(s) = &got {
                *idents.entry(s.clone()).or_default() += 1;
            }
            let key = format!("{k}  {:<10} | expected {}", tok_kind(&got), expected);
            *groups.entry(key.clone()).or_default() += 1;
            let ex = examples.entry(key).or_default();
            if ex.len() < 3 {
                ex.push(path.file_name().unwrap().to_string_lossy().to_string());
            }
        }
    }

    println!("================ translate-stage parser failures: {total} ================\n");
    let mut v: Vec<_> = groups.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    for (k, n) in &v {
        println!("  {n:>4}  {k}");
        if let Some(ex) = examples.get(*k) {
            for e in ex {
                println!("          e.g. {e}");
            }
        }
    }

    println!("\n-- specific Ident tokens (top 20) --");
    let mut iv: Vec<_> = idents.iter().collect();
    iv.sort_by(|a, b| b.1.cmp(a.1));
    for (s, n) in iv.iter().take(20) {
        println!("  {n:>4}  Ident({s})");
    }
}
