//! Triage helper (analysis only): deep-drill the `InvalidStoreTypes` validate
//! bucket. For every shader that translates + naga-parses but fails validation
//! with `InvalidStoreTypes`, it slices the offending store statement out of the
//! generated WGSL (via the error's narrowest span), classifies it by the width
//! of the assignment target vs. the assigned value, and prints representative
//! examples per subgroup.
//!
//! ```text
//! cargo run -p pm-preset --example store_drill --release -- <dir>...
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

/// Strip one layer of balanced outer parens, repeatedly.
fn unwrap_parens(s: &str) -> &str {
    let mut e = s.trim();
    while e.starts_with('(') && e.ends_with(')') {
        let inner = &e[1..e.len() - 1];
        // confirm the first '(' matches the last ')'
        let mut depth = 0i32;
        let mut ok = true;
        for (i, c) in inner.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        ok = false;
                        break;
                    }
                }
                _ => {}
            }
            let _ = i;
        }
        if ok && depth == 0 {
            e = inner.trim();
        } else {
            break;
        }
    }
    e
}

/// Split at the last top-level (depth-0) occurrence of any binary operator.
fn split_binop(e: &str) -> Option<(&str, &str)> {
    let bytes = e.as_bytes();
    let mut depth = 0i32;
    let mut i = e.len();
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b')' => depth += 1,
            b'(' => depth -= 1,
            b'+' | b'-' | b'*' | b'/' if depth == 0 && i > 0 && i + 1 < e.len() => {
                // require surrounding spaces to avoid unary/`->`
                if bytes[i - 1] == b' ' && bytes[i + 1] == b' ' {
                    return Some((e[..i].trim(), e[i + 1..].trim()));
                }
            }
            _ => {}
        }
    }
    None
}

/// Width of a WGSL expression fragment, if recognizable (1=scalar..4=vec4).
/// `wgsl` lets bare identifiers resolve to their declared width.
fn width(expr: &str, wgsl: &str) -> Option<u8> {
    let e = unwrap_parens(expr.trim().trim_end_matches(';'));
    // trailing swizzle: `.xyz`, `.xy`, `.x` ...
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
    if e.starts_with("f32(") || e.starts_with("i32(") {
        return Some(1);
    }
    if e.parse::<f64>().is_ok() {
        return Some(1);
    }
    // binary expression: widest recognized operand (broadcast / mat*vec ~ widest)
    if let Some((a, b)) = split_binop(e) {
        let wa = width(a, wgsl);
        let wb = width(b, wgsl);
        return match (wa, wb) {
            (Some(x), Some(y)) => Some(x.max(y)),
            (Some(x), None) | (None, Some(x)) => Some(x),
            _ => None,
        };
    }
    // bare identifier -> declared width.
    if e.chars().all(|c| c.is_alphanumeric() || c == '_') && !e.is_empty() {
        return declared_width(wgsl, e);
    }
    None
}

/// Look up a declared variable's width from the WGSL (`var NAME: TYPE`).
fn declared_width(wgsl: &str, name: &str) -> Option<u8> {
    for kw in ["var<private> ", "var ", "let "] {
        let needle = format!("{kw}{name}: ");
        if let Some(p) = wgsl.find(&needle) {
            let ty = &wgsl[p + needle.len()..];
            return if ty.starts_with("vec4<") || ty.starts_with("mat") {
                Some(4)
            } else if ty.starts_with("vec3<") {
                Some(3)
            } else if ty.starts_with("vec2<") {
                Some(2)
            } else if ty.starts_with("f32") || ty.starts_with("i32") || ty.starts_with("bool") {
                Some(1)
            } else {
                None
            };
        }
    }
    None
}

fn wname(w: Option<u8>) -> &'static str {
    match w {
        Some(1) => "scalar",
        Some(2) => "vec2",
        Some(3) => "vec3",
        Some(4) => "vec4/mat",
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

    let mut total = 0usize;
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut unparsed: Vec<String> = Vec::new();

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
            if !format!("{e:?}").contains("InvalidStoreTypes") {
                continue;
            }
            total += 1;

            // Narrowest span = the offending store statement.
            let mut best: Option<(usize, usize)> = None;
            for sc in e.spans() {
                if let Some(r) = sc.0.to_range() {
                    let len = r.end - r.start;
                    if len > 0 && best.map(|(s, en)| len < en - s).unwrap_or(true) {
                        best = Some((r.start, r.end));
                    }
                }
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let Some((s, en)) = best else {
                unparsed.push(format!("{name} [{kind:?}] <no span>"));
                continue;
            };
            // Expand the narrow value/pointer span to its enclosing statement
            // (`lhs = rhs;`) so we can see both sides of the store.
            let ls = t.wgsl[..s].rfind(['\n', ';', '{', '}']).map(|p| p + 1).unwrap_or(0);
            let le = t.wgsl[en..].find(';').map(|p| en + p).unwrap_or(en);
            let stmt = t.wgsl[ls..le].replace('\n', " ").trim().to_string();

            // Split `lhs = rhs` at the first top-level ` = `.
            let assign = stmt.find(" = ");
            let (lw, rw, rot) = if let Some(p) = assign {
                let lhs = stmt[..p].trim();
                let rhs = stmt[p + 3..].trim();
                let lw = width(lhs, &t.wgsl);
                let rot = rhs.contains("rot_") && rhs.contains('*');
                (lw, width(rhs, &t.wgsl), rot)
            } else {
                (None, None, false)
            };
            let key = if rot {
                format!("{:>6} <- rot-matrix*v", wname(lw))
            } else {
                format!("{:>6} <- {:<6}", wname(lw), wname(rw))
            };
            *groups.entry(key.clone()).or_default() += 1;
            let ex = examples.entry(key).or_default();
            if ex.len() < 3 {
                let short = if stmt.len() > 140 { format!("{}…", &stmt[..140]) } else { stmt.clone() };
                ex.push(format!("{name} [{kind:?}]  {short}"));
            }
        }
    }

    println!("================ InvalidStoreTypes : {total} shaders ================");
    println!("(target <- value, by recognized width)\n");
    let mut v: Vec<_> = groups.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    for (k, n) in &v {
        println!("  {n:>4}  {k}");
    }
    println!("\n-- representative offending stores --");
    for (k, _) in &v {
        if let Some(ex) = examples.get(*k) {
            println!("[{k}]");
            for line in ex {
                println!("    {line}");
            }
        }
    }
    if !unparsed.is_empty() {
        println!("\n-- no usable span ({}) --", unparsed.len());
        for u in unparsed.iter().take(5) {
            println!("    {u}");
        }
    }
}
