//! A minimal C-style preprocessor for Milkdrop preset shaders.
//!
//! Mirrors the `ApplyPreprocessor` step of projectM's HLSL pipeline well enough
//! for real presets: it strips comments, joins line continuations, and expands
//! object-like and function-like `#define` macros (the preset shader header is
//! built almost entirely from these — `time`, `bass`, `q1`, `GetBlur1`, `lum`,
//! …). Conditional directives (`#ifdef` etc.) are uncommon in presets and are
//! dropped.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
struct Macro {
    /// `None` for object-like macros; `Some(params)` for function-like.
    params: Option<Vec<String>>,
    body: String,
}

/// Run the preprocessor, returning fully macro-expanded source.
pub fn preprocess(src: &str) -> String {
    let src = strip_comments(src);
    let src = join_line_continuations(&src);
    let (body, macros) = extract_defines(&src);
    expand_str(&body, &macros, &HashSet::new())
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Replace `//` and `/* */` comments with spaces, preserving newlines so line
/// structure (and thus `#define` parsing) is unaffected.
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                // Keep newlines so directive lines stay separated.
                if b[i] == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i += 2;
            out.push(' ');
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

fn join_line_continuations(src: &str) -> String {
    src.replace("\\\n", "").replace("\\\r\n", "")
}

/// Pull out `#define`s and return the source with directive lines removed.
fn extract_defines(src: &str) -> (String, HashMap<String, Macro>) {
    let mut macros = HashMap::new();
    let mut body = String::with_capacity(src.len());

    for line in src.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let rest = rest.trim_start();
            if let Some(def) = rest.strip_prefix("define") {
                if let Some((name, m)) = parse_define(def) {
                    macros.insert(name, m);
                }
            }
            // All other directives are dropped (line removed).
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }

    (body, macros)
}

/// Parse the text following `#define`.
fn parse_define(def: &str) -> Option<(String, Macro)> {
    let b = def.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    let name_start = i;
    while i < b.len() && is_ident_continue(b[i]) {
        i += 1;
    }
    if i == name_start {
        return None;
    }
    let name = def[name_start..i].to_string();

    // Function-like only if '(' immediately follows the name (no space).
    if i < b.len() && b[i] == b'(' {
        i += 1;
        let mut params = Vec::new();
        let mut cur = String::new();
        while i < b.len() && b[i] != b')' {
            let c = b[i] as char;
            if c == ',' {
                params.push(cur.trim().to_string());
                cur.clear();
            } else {
                cur.push(c);
            }
            i += 1;
        }
        if i < b.len() {
            i += 1; // consume ')'
        }
        let last = cur.trim();
        if !last.is_empty() {
            params.push(last.to_string());
        }
        let body = def[i..].trim().to_string();
        Some((name, Macro { params: Some(params), body }))
    } else {
        let body = def[i..].trim().to_string();
        Some((name, Macro { params: None, body }))
    }
}

/// Expand all macros in `s`. `active` guards against recursive self-expansion.
fn expand_str(s: &str, macros: &HashMap<String, Macro>, active: &HashSet<String>) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;

    while i < b.len() {
        if is_ident_start(b[i]) {
            let start = i;
            while i < b.len() && is_ident_continue(b[i]) {
                i += 1;
            }
            let ident = &s[start..i];

            match macros.get(ident) {
                Some(m) if !active.contains(ident) => match &m.params {
                    None => {
                        let mut next_active = active.clone();
                        next_active.insert(ident.to_string());
                        out.push_str(&expand_str(&m.body, macros, &next_active));
                    }
                    Some(params) => {
                        // Function-like: only expand if a '(' follows.
                        let mut k = i;
                        while k < b.len() && (b[k] == b' ' || b[k] == b'\t' || b[k] == b'\n') {
                            k += 1;
                        }
                        if k < b.len() && b[k] == b'(' {
                            if let Some((args, end)) = parse_call_args(b, k) {
                                let substituted = substitute(&m.body, params, &args, macros, active);
                                let mut next_active = active.clone();
                                next_active.insert(ident.to_string());
                                out.push_str(&expand_str(&substituted, macros, &next_active));
                                i = end;
                            } else {
                                out.push_str(ident);
                            }
                        } else {
                            out.push_str(ident);
                        }
                    }
                },
                _ => out.push_str(ident),
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }

    out
}

/// Parse a function-macro argument list starting at the `(` index. Returns the
/// argument strings (split on top-level commas) and the index past the `)`.
fn parse_call_args(b: &[u8], open_paren: usize) -> Option<(Vec<String>, usize)> {
    let mut i = open_paren + 1;
    let mut depth = 1i32;
    let mut args = Vec::new();
    let mut cur = String::new();

    while i < b.len() {
        let c = b[i] as char;
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    i += 1;
                    break;
                }
                cur.push(c);
            }
            ',' if depth == 1 => {
                args.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
        i += 1;
    }

    if depth != 0 {
        return None; // unterminated
    }
    let last = cur.trim();
    if !last.is_empty() || !args.is_empty() {
        args.push(last.to_string());
    }
    Some((args, i))
}

/// Substitute macro parameters in `body` with the (pre-expanded) argument text.
fn substitute(
    body: &str,
    params: &[String],
    args: &[String],
    macros: &HashMap<String, Macro>,
    active: &HashSet<String>,
) -> String {
    // Arguments are macro-expanded before substitution, per C semantics.
    let expanded_args: Vec<String> =
        args.iter().map(|a| expand_str(a, macros, active)).collect();

    let b = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < b.len() {
        if is_ident_start(b[i]) {
            let start = i;
            while i < b.len() && is_ident_continue(b[i]) {
                i += 1;
            }
            let ident = &body[start..i];
            if let Some(pos) = params.iter().position(|p| p == ident) {
                out.push_str(expanded_args.get(pos).map(String::as_str).unwrap_or(""));
            } else {
                out.push_str(ident);
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pp(s: &str) -> String {
        // Collapse whitespace for stable comparisons.
        preprocess(s).split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn object_like_define() {
        let src = "#define time _c2.x\nfloat t = time;";
        assert_eq!(pp(src), "float t = _c2.x;");
    }

    #[test]
    fn function_like_define() {
        let src = "#define lum(x) (dot(x,float3(0.32,0.49,0.29)))\nfloat l = lum(color);";
        assert_eq!(pp(src), "float l = (dot(color,float3(0.32,0.49,0.29)));");
    }

    #[test]
    fn nested_macro_expansion() {
        // GetBlur1 expands to a tex2D call referencing _c5, like the real header.
        let src = "#define GetBlur1(uv) (tex2D(sampler_blur1,uv).xyz*_c5.x + _c5.y)\n\
                   float3 b = GetBlur1(texCoord);";
        assert_eq!(pp(src), "float3 b = (tex2D(sampler_blur1,texCoord).xyz*_c5.x + _c5.y);");
    }

    #[test]
    fn recursive_object_macros() {
        let src = "#define A B\n#define B 42\nint x = A;";
        assert_eq!(pp(src), "int x = 42;");
    }

    #[test]
    fn self_reference_is_not_infinite() {
        let src = "#define x x\nint y = x;";
        assert_eq!(pp(src), "int y = x;");
    }

    #[test]
    fn comments_stripped_before_expansion() {
        let src = "#define time _c2.x\nfloat t = time; // time is here\nfloat u = /* x */ time;";
        assert_eq!(pp(src), "float t = _c2.x; float u = _c2.x;");
    }

    #[test]
    fn arg_with_nested_parens() {
        let src = "#define f(a) [a]\nint z = f(g(1,2));";
        assert_eq!(pp(src), "int z = [g(1,2)];");
    }
}
