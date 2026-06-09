//! Triage helper (analysis only): deep-drill Bucket F (naga "Unexpected
//! runtime-expression" — module-scope global declarations whose initializer is
//! not a const-expression), plus dump the 9-shader Bucket E `cannot cast` tail.
//!
//! For every shader that translates but fails naga *parse* with a runtime-expr
//! error, it re-parses the wrapped HLSL to the pm-shader AST and classifies the
//! offending globals by:
//!   * initializer form (scalar / vector ctor / matrix ctor / call / other-expr),
//!   * dependency (references a uniform / another preset global / a call),
//!   * read-only vs mutated later,
//!   * referenced inside a helper function vs only in the `PS` entry.
//!
//! Then it gives each shader a lowering verdict (trivial PS-local `let` / needs
//! `var` / helper-ref-hard / inter-global-ordered).
//!
//! ```text
//! cargo run -p pm-preset --example bucket_f_drill --release -- <dir>...
//! ```

use pm_preset::{shader_to_wgsl, wrap_shader, Preset, ShaderKind};
use pm_shader::{parse, preprocess, Expr, Item, Stmt, Type};
use std::collections::{BTreeMap, HashSet};
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

/// Naga parse error message (empty string if it parses OK).
fn parse_error(wgsl: &str) -> String {
    match naga::front::wgsl::parse_str(wgsl) {
        Ok(_) => String::new(),
        Err(e) => e.emit_to_string(wgsl),
    }
}

/// Collect every identifier read anywhere in an expression.
fn idents(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Ident(n) => {
            out.insert(n.clone());
        }
        Expr::Unary(_, a) | Expr::PostInc(a) | Expr::PostDec(a) | Expr::PreInc(a)
        | Expr::PreDec(a) | Expr::Member(a, _) => idents(a, out),
        Expr::Binary(_, a, b) | Expr::Index(a, b) => {
            idents(a, out);
            idents(b, out);
        }
        Expr::Assign(_, a, b) => {
            idents(a, out);
            idents(b, out);
        }
        Expr::Ternary(a, b, c) => {
            idents(a, out);
            idents(b, out);
            idents(c, out);
        }
        Expr::Call(_, args) | Expr::Construct(_, args) => {
            for a in args {
                idents(a, out);
            }
        }
        _ => {}
    }
}

/// Does an expression contain any function call?
fn has_call(e: &Expr) -> bool {
    match e {
        Expr::Call(..) => true,
        Expr::Unary(_, a) | Expr::PostInc(a) | Expr::PostDec(a) | Expr::PreInc(a)
        | Expr::PreDec(a) | Expr::Member(a, _) => has_call(a),
        Expr::Binary(_, a, b) | Expr::Index(a, b) | Expr::Assign(_, a, b) => has_call(a) || has_call(b),
        Expr::Ternary(a, b, c) => has_call(a) || has_call(b) || has_call(c),
        Expr::Construct(_, args) => args.iter().any(has_call),
        _ => false,
    }
}

/// Root identifier of an lvalue (peel `.member` / `[index]`).
fn root_ident(e: &Expr) -> Option<&str> {
    match e {
        Expr::Ident(n) => Some(n),
        Expr::Member(a, _) | Expr::Index(a, _) => root_ident(a),
        _ => None,
    }
}

/// Walk statements, collecting (a) all idents read and (b) roots assigned-to.
fn walk_stmt(s: &Stmt, reads: &mut HashSet<String>, assigned: &mut HashSet<String>) {
    match s {
        Stmt::Decl { init, .. } => {
            if let Some(e) = init {
                idents(e, reads);
            }
        }
        Stmt::Expr(e) => {
            idents(e, reads);
            match e {
                Expr::Assign(_, lhs, _) => {
                    if let Some(r) = root_ident(lhs) {
                        assigned.insert(r.to_string());
                    }
                }
                Expr::PostInc(a) | Expr::PostDec(a) | Expr::PreInc(a) | Expr::PreDec(a) => {
                    if let Some(r) = root_ident(a) {
                        assigned.insert(r.to_string());
                    }
                }
                _ => {}
            }
        }
        Stmt::If(c, t, e) => {
            idents(c, reads);
            walk_stmt(t, reads, assigned);
            if let Some(e) = e {
                walk_stmt(e, reads, assigned);
            }
        }
        Stmt::For(init, cond, step, body) => {
            if let Some(i) = init {
                walk_stmt(i, reads, assigned);
            }
            if let Some(c) = cond {
                idents(c, reads);
            }
            if let Some(s) = step {
                idents(s, reads);
                if let Some(r) = root_ident(s) {
                    if matches!(s, Expr::Assign(..) | Expr::PostInc(_) | Expr::PreInc(_) | Expr::PostDec(_) | Expr::PreDec(_)) {
                        assigned.insert(r.to_string());
                    }
                }
            }
            walk_stmt(body, reads, assigned);
        }
        Stmt::While(c, body) => {
            idents(c, reads);
            walk_stmt(body, reads, assigned);
        }
        Stmt::Return(Some(e)) => idents(e, reads),
        Stmt::Return(None) => {}
        Stmt::Block(b) => {
            for s in b {
                walk_stmt(s, reads, assigned);
            }
        }
    }
}

fn type_class(ty: Type) -> &'static str {
    match ty {
        Type::Mat2 | Type::Mat3 | Type::Mat4 | Type::Mat3x4 | Type::Mat4x3 => "matrix",
        Type::Float2 | Type::Float3 | Type::Float4 | Type::Int2 | Type::Int3 | Type::Int4
        | Type::Bool2 | Type::Bool3 | Type::Bool4 => "vector",
        _ => "scalar",
    }
}

fn bump(m: &mut BTreeMap<&'static str, usize>, k: &'static str) {
    *m.entry(k).or_default() += 1;
}

fn main() {
    let dirs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let mut files = Vec::new();
    for d in &dirs {
        collect(d, &mut files);
    }
    files.sort();

    let (mut f_warp, mut f_comp) = (0usize, 0usize);
    let mut g_total = 0usize; // runtime-offender globals
    let mut init_form: BTreeMap<&str, usize> = BTreeMap::new();
    let mut dep: BTreeMap<&str, usize> = BTreeMap::new();
    let mut mutated = 0usize;
    let mut helper_ref = 0usize;
    // per-shader verdict
    let mut v_trivial = 0usize; // all read-only, PS-only -> PS-local `let`
    let mut v_needs_var = 0usize; // some mutated (still PS-only)
    let mut v_helper = 0usize; // some offender used in a helper fn
    let mut v_interglobal = 0usize; // some offender init references another global
    let mut e_tail: Vec<String> = Vec::new();

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
            let err = parse_error(&t.wgsl);
            if err.is_empty() {
                continue;
            }
            // Bucket E tail.
            if err.lines().any(|l| l.contains("cannot cast")) {
                let line = err
                    .lines()
                    .find(|l| l.contains("cannot cast"))
                    .unwrap_or("")
                    .trim();
                e_tail.push(format!("{}  [{:?}]  {}", path.file_name().unwrap().to_string_lossy(), kind, line));
                continue;
            }
            if !err.contains("runtime-expression") {
                continue;
            }
            // Bucket F. Count + classify via the AST.
            match kind {
                ShaderKind::Warp => f_warp += 1,
                ShaderKind::Composite => f_comp += 1,
            }
            let Ok(wrapped) = wrap_shader(src, kind) else { continue };
            let pre = preprocess(&wrapped);
            let Ok(items) = parse(&pre) else { continue };

            let mut uniforms: HashSet<String> = HashSet::new();
            let mut globals: Vec<(String, Type, Expr)> = Vec::new();
            let mut global_names: HashSet<String> = HashSet::new();
            for it in &items {
                if let Item::Global { uniform, ty, name, init, .. } = it {
                    if *uniform {
                        uniforms.insert(name.clone());
                    } else {
                        global_names.insert(name.clone());
                        if let Some(e) = init {
                            globals.push((name.clone(), *ty, e.clone()));
                        }
                    }
                }
            }
            // reads/assigns per function, split PS vs helpers.
            let mut helper_reads: HashSet<String> = HashSet::new();
            let mut all_assigned: HashSet<String> = HashSet::new();
            for it in &items {
                if let Item::Function(f) = it {
                    let mut reads = HashSet::new();
                    let mut assigned = HashSet::new();
                    for s in &f.body {
                        walk_stmt(s, &mut reads, &mut assigned);
                    }
                    all_assigned.extend(assigned.iter().cloned());
                    if f.name != "PS" {
                        helper_reads.extend(reads);
                    }
                }
            }

            let (mut sh_mut, mut sh_helper, mut sh_interg) = (false, false, false);
            let mut sh_offenders = 0usize;
            for (name, ty, init) in &globals {
                // is this a runtime-offender? init references a uniform / another
                // global, or contains a call (non-const in WGSL).
                let mut ids = HashSet::new();
                idents(init, &mut ids);
                let uses_uniform = ids.iter().any(|i| uniforms.contains(i));
                let uses_global = ids.iter().any(|i| global_names.contains(i) && i != name);
                let uses_call = has_call(init);
                if !(uses_uniform || uses_global || uses_call) {
                    continue; // const initializer, fine
                }
                sh_offenders += 1;
                g_total += 1;

                // init form
                let form = match init {
                    Expr::Construct(cty, _) => type_class(*cty),
                    Expr::Call(..) => "call",
                    _ => {
                        // tag by declared type, mark as plain expr
                        match type_class(*ty) {
                            "matrix" => "expr(matrix)",
                            "vector" => "expr(vector)",
                            _ => "expr(scalar)",
                        }
                    }
                };
                bump(&mut init_form, form);
                if uses_uniform {
                    bump(&mut dep, "uses-uniform");
                }
                if uses_global {
                    bump(&mut dep, "uses-other-global");
                    sh_interg = true;
                }
                if uses_call && !uses_uniform && !uses_global {
                    bump(&mut dep, "uses-call-only");
                }
                if all_assigned.contains(name) {
                    mutated += 1;
                    sh_mut = true;
                }
                if helper_reads.contains(name) {
                    helper_ref += 1;
                    sh_helper = true;
                }
            }
            if sh_offenders == 0 {
                continue;
            }
            if sh_helper {
                v_helper += 1;
            } else if sh_mut {
                v_needs_var += 1;
            } else if sh_interg {
                v_interglobal += 1;
            } else {
                v_trivial += 1;
            }
        }
    }

    let f_total = f_warp + f_comp;
    println!("================ BUCKET F (runtime-expr global init) ================");
    println!("F shaders: {f_total}  (warp {f_warp}, comp {f_comp})");
    println!("Runtime-offender globals across those shaders: {g_total}");
    println!("  mutated later:                {mutated}");
    println!("  referenced in a helper fn:    {helper_ref}");
    println!();
    println!("-- offender initializer form --");
    for (k, n) in &init_form {
        println!("  {n:>5}  {k}");
    }
    println!("-- offender dependency (non-exclusive) --");
    for (k, n) in &dep {
        println!("  {n:>5}  {k}");
    }
    println!();
    println!("-- per-shader lowering verdict (mutually exclusive, priority helper>var>interglobal>trivial) --");
    println!("  {v_trivial:>5}  TRIVIAL    (all offenders read-only, PS-only, no inter-global dep -> PS-local `let`)");
    println!("  {v_interglobal:>5}  ORDERED    (read-only, PS-only, but an offender depends on another global -> ordered `let`s)");
    println!("  {v_needs_var:>5}  NEEDS-VAR  (an offender is mutated later, still PS-only -> PS-local `var`)");
    println!("  {v_helper:>5}  HELPER-REF (an offender is read inside a helper fn -> NOT a simple PS-local; hard)");
    println!();
    println!("================ BUCKET E TAIL (cannot cast) : {} ================", e_tail.len());
    for line in &e_tail {
        println!("  {line}");
    }
}
