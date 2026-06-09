//! Triage helper (analysis only): bin every naga *validation* failure by its
//! innermost error variant, and subgroup the two largest:
//!   * `InvalidReturnType` — by declared return type (`-> T`) vs. the returned
//!     expression's recognized width (or "valueless" when there's no value).
//!   * `Expression / InvalidBooleanVector` — the `all()`/`any()`-on-numeric
//!     family — with the offending call sliced from the WGSL.
//!
//! ```text
//! cargo run -p pm-preset --example validate_drill --release -- <dir>...
//! ```

use pm_preset::{shader_to_wgsl, Preset, ShaderKind};
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

/// Innermost `source: Variant` name in the Debug string.
fn variant(dbg: &str) -> String {
    match dbg.rfind("source: ") {
        Some(p) => {
            let rest = &dbg[p + 8..];
            rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect()
        }
        None => "?".into(),
    }
}

/// Recognized width of a WGSL expression fragment (1..4), best-effort.
fn width(e: &str) -> Option<u8> {
    let e = e.trim().trim_end_matches(';').trim();
    if let Some(dot) = e.rfind('.') {
        let tail = &e[dot + 1..];
        if !tail.is_empty() && tail.len() <= 4 && tail.chars().all(|c| matches!(c, 'x' | 'y' | 'z' | 'w' | 'r' | 'g' | 'b' | 'a')) {
            return Some(tail.len() as u8);
        }
    }
    if e.starts_with("vec4<") || e.starts_with("textureSample(") {
        return Some(4);
    }
    if e.starts_with("vec3<") {
        return Some(3);
    }
    if e.starts_with("vec2<") {
        return Some(2);
    }
    if e.starts_with("f32(") || e.parse::<f64>().is_ok() {
        return Some(1);
    }
    None
}

fn wname(w: Option<u8>) -> &'static str {
    match w {
        Some(1) => "scalar",
        Some(2) => "vec2",
        Some(3) => "vec3",
        Some(4) => "vec4",
        _ => "?",
    }
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let mut files = Vec::new();
    for d in &dirs {
        collect(d, &mut files);
    }
    files.sort();

    let mut variants: BTreeMap<String, usize> = BTreeMap::new();
    let mut ret_groups: BTreeMap<String, usize> = BTreeMap::new();
    let mut ret_ex: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut boolvec_ex: Vec<String> = Vec::new();

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
            let Ok(module) = naga::front::wgsl::parse_str(&t.wgsl) else { continue };
            let mut v = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            );
            let Err(e) = v.validate(&module) else { continue };
            let dbg = format!("{e:?}");
            let var = variant(&dbg);
            *variants.entry(var.clone()).or_default() += 1;
            let name = path.file_name().unwrap().to_string_lossy().to_string();

            // span ranges (sorted by length): largest ~ function, smallest ~ expr.
            let mut ranges: Vec<(usize, usize)> = e
                .spans()
                .filter_map(|sc| sc.0.to_range())
                .filter(|r| r.end > r.start)
                .map(|r| (r.start, r.end))
                .collect();
            ranges.sort_by_key(|(s, en)| en - s);

            if var == "InvalidReturnType" {
                // declared return type: from the largest span's `-> T {`
                let decl = ranges
                    .last()
                    .and_then(|&(s, en)| t.wgsl.get(s..en))
                    .and_then(|f| f.find("-> ").map(|p| &f[p + 3..]))
                    .map(|r| r.split([' ', '{', '\n']).next().unwrap_or("?").to_string())
                    .unwrap_or_else(|| "?".into());
                // returned expression: smallest span if there are >=2 spans, else None.
                let (val, vw) = if ranges.len() >= 2 {
                    let (s, en) = ranges[0];
                    let txt = t.wgsl[s..en].replace('\n', " ").trim().to_string();
                    (txt.clone(), wname(width(&txt)))
                } else {
                    ("<valueless / fallthrough>".into(), "none")
                };
                let dw = if decl.starts_with("vec4") { "vec4" } else if decl.starts_with("vec3") { "vec3" } else if decl.starts_with("vec2") { "vec2" } else if decl.starts_with("f32") { "scalar" } else { decl.as_str() };
                let key = format!("decl {dw:<6} <- ret {vw}");
                *ret_groups.entry(key.clone()).or_default() += 1;
                let ex = ret_ex.entry(key).or_default();
                if ex.len() < 3 {
                    let short = if val.len() > 90 { format!("{}…", &val[..90]) } else { val };
                    ex.push(format!("{name} [{kind:?}] -> {decl}  returns: {short}"));
                }
            } else if var == "InvalidBooleanVector" && boolvec_ex.len() < 6 {
                {
                    let line = ranges
                        .first()
                        .and_then(|&(s, en)| {
                            let ls = t.wgsl[..s].rfind(['\n', '{', ';']).map(|p| p + 1).unwrap_or(0);
                            let le = t.wgsl[en..].find([';', '\n']).map(|p| en + p).unwrap_or(en);
                            t.wgsl.get(ls..le)
                        })
                        .unwrap_or("")
                        .replace('\n', " ")
                        .trim()
                        .to_string();
                    boolvec_ex.push(format!("{name} [{kind:?}]  {line}"));
                }
            }
        }
    }

    println!("================ validation failures by innermost variant ================");
    let mut vv: Vec<_> = variants.iter().collect();
    vv.sort_by(|a, b| b.1.cmp(a.1));
    for (k, n) in &vv {
        println!("  {n:>4}  {k}");
    }

    println!("\n================ InvalidReturnType subgroups ================");
    let mut rv: Vec<_> = ret_groups.iter().collect();
    rv.sort_by(|a, b| b.1.cmp(a.1));
    for (k, n) in &rv {
        println!("  {n:>4}  {k}");
    }
    println!("-- representative returns --");
    for (k, _) in &rv {
        if let Some(ex) = ret_ex.get(*k) {
            println!("[{k}]");
            for l in ex {
                println!("    {l}");
            }
        }
    }

    println!("\n================ InvalidBooleanVector (all/any on numeric) ================");
    for l in &boolvec_ex {
        println!("    {l}");
    }
}
