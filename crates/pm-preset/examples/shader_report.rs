//! Shader compatibility report: translate every preset's warp & composite
//! shaders to WGSL and validate with naga.
//!
//! ```text
//! cargo run -p pm-preset --example shader_report --release -- <dir> [<dir>...]
//! ```

use pm_preset::{shader_to_wgsl, Preset, ShaderKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Tally {
    attempted: usize,
    ok: usize,
    translate_fail: usize,
    validate_fail: usize,
    buckets: HashMap<String, usize>,
    samples: HashMap<String, String>,
}

impl Tally {
    fn note(&mut self, bucket: &str, sample: &str) {
        *self.buckets.entry(bucket.to_string()).or_default() += 1;
        self.samples.entry(bucket.to_string()).or_insert_with(|| sample.to_string());
    }
}

fn validate_wgsl(wgsl: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| {
        let first = e.emit_to_string(wgsl).lines().next().unwrap_or("").trim().to_string();
        format!("parse: {first}")
    })?;
    let mut v = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    v.validate(&module).map_err(|e| {
        format!("validate: {e:?}").chars().take(80).collect::<String>()
    })?;
    Ok(())
}

fn check(tally: &mut Tally, name: &str, src: &str, kind: ShaderKind) {
    if !src.contains("shader_body") {
        return; // no custom shader of this kind
    }
    tally.attempted += 1;
    match shader_to_wgsl(src, kind) {
        Err(e) => {
            tally.translate_fail += 1;
            let b = format!("translate: {}", first_words(&e.to_string()));
            tally.note(&b, name);
        }
        Ok(wgsl) => match validate_wgsl(&wgsl) {
            Ok(()) => tally.ok += 1,
            Err(msg) => {
                tally.validate_fail += 1;
                tally.note(&format!("naga {}", first_words(&msg)), name);
            }
        },
    }
}

fn first_words(s: &str) -> String {
    s.split_whitespace().take(6).collect::<Vec<_>>().join(" ")
}

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

fn print_tally(label: &str, t: &Tally) {
    let pct = |n: usize| if t.attempted > 0 { 100.0 * n as f64 / t.attempted as f64 } else { 0.0 };
    println!("\n===== {label} shaders =====");
    println!("Attempted (have shader_body): {}", t.attempted);
    println!("Valid WGSL:        {}  ({:.1}%)", t.ok, pct(t.ok));
    println!("Translate failed:  {}  ({:.1}%)", t.translate_fail, pct(t.translate_fail));
    println!("naga rejected:     {}  ({:.1}%)", t.validate_fail, pct(t.validate_fail));
    let mut buckets: Vec<_> = t.buckets.iter().collect();
    buckets.sort_by(|a, b| b.1.cmp(a.1));
    for (bucket, count) in buckets.iter().take(12) {
        println!("  {count:>6}  {bucket}");
        if let Some(s) = t.samples.get(*bucket) {
            println!("            e.g. {}", Path::new(s).file_name().unwrap().to_string_lossy());
        }
    }
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if dirs.is_empty() {
        eprintln!("usage: shader_report <dir> [<dir>...]");
        std::process::exit(2);
    }
    let mut files = Vec::new();
    for d in &dirs {
        collect(d, &mut files);
    }
    files.sort();
    println!("Scanning {} presets for custom shaders...", files.len());

    let mut warp = Tally::default();
    let mut comp = Tally::default();
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let content = String::from_utf8_lossy(&bytes);
        let Ok(preset) = Preset::load(&content) else { continue };
        let name = path.file_name().unwrap().to_string_lossy();
        check(&mut warp, &name, preset.warp_shader_source(), ShaderKind::Warp);
        check(&mut comp, &name, preset.composite_shader_source(), ShaderKind::Composite);
    }

    print_tally("WARP", &warp);
    print_tally("COMPOSITE", &comp);
}
