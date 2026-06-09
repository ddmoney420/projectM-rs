//! WGSL code generation from the HLSL AST.
//!
//! WGSL is much stricter than HLSL, so this is more than a syntactic rewrite.
//! The notable transforms:
//!   * scalar → vector promotion (`float3 ret = 0;` ⇒ `vec3<f32>(0.0)`),
//!   * vec/scalar operands broadcast explicitly (WGSL forbids `vec + scalar`),
//!   * `cond ? a : b` ⇒ `select(b, a, cond)` (WGSL has no ternary),
//!   * multi-component swizzle assignment (`v.xy = e`) expanded component-wise,
//!   * `out` parameters lifted into a returned struct (WGSL has no `out`),
//!   * intrinsic remapping (`lerp`→`mix`, `frac`→`fract`, `saturate`→`clamp`,
//!     `tex2D`→`textureSample`, …).
//!
//! Global uniforms are emitted as `var<private>` so a translated shader is a
//! self-contained, naga-validatable module; the real uniform/texture binding
//! layout is applied later by the preset engine (Phase 5).

use crate::ast::*;
use std::collections::HashMap;
use std::fmt::Write;

pub fn generate(items: &[Item]) -> String {
    let mut g = Generator::new(items);
    g.run(items);
    g.out
}

struct Generator {
    globals: HashMap<String, Type>,
    locals: HashMap<String, Type>,
    /// User-defined function signatures `(return type, parameter types)`, so
    /// calls infer correctly and arguments coerce to the declared param types.
    functions: HashMap<String, (Type, Vec<Type>)>,
    /// Declared return type of the function currently being emitted, so a
    /// `return` can coerce its expression to the declared type.
    current_ret: Type,
    out: String,
    temp: usize,
}

impl Generator {
    fn new(items: &[Item]) -> Self {
        let mut globals = HashMap::new();
        let mut functions = HashMap::new();
        for item in items {
            match item {
                Item::Global { ty, name, .. } => {
                    globals.insert(name.clone(), *ty);
                }
                Item::Function(f) => {
                    let params = f.params.iter().map(|p| p.ty).collect();
                    functions.insert(f.name.clone(), (f.ret, params));
                }
                _ => {}
            }
        }
        Generator { globals, locals: HashMap::new(), functions, current_ret: Type::Void, out: String::new(), temp: 0 }
    }

    fn run(&mut self, items: &[Item]) {
        // Module-scope globals as private vars (placeholder for uniforms).
        for item in items {
            if let Item::Global { ty, name, init, .. } = item {
                let init_s = match init {
                    Some(e) => format!(" = {}", self.expr(e, *ty)),
                    None => String::new(),
                };
                let _ = writeln!(self.out, "var<private> {}: {}{};", sanitize_ident(name), wgsl_type(*ty), init_s);
            }
        }
        // Textures/samplers: note the binding convention for the preset engine.
        for item in items {
            if let Item::Sampler { ty, name } = item {
                let dim = if *ty == Type::Sampler3D { "texture_3d<f32>" } else { "texture_2d<f32>" };
                let _ = writeln!(self.out, "// sampler {name}: bind `{name}` as {dim} and `{name}_sampler` as sampler");
            }
        }
        if !self.out.is_empty() {
            self.out.push('\n');
        }
        for item in items {
            if let Item::Function(f) = item {
                self.function(f);
            }
        }
    }

    fn fresh_temp(&mut self) -> String {
        let t = format!("_pm_tmp{}", self.temp);
        self.temp += 1;
        t
    }

    // -------------------------------------------------------- functions ------

    fn function(&mut self, f: &Function) {
        self.locals.clear();
        self.current_ret = f.ret;
        let out_params: Vec<&Param> = f.params.iter().filter(|p| p.qualifier == ParamQual::Out).collect();
        let in_params: Vec<&Param> = f.params.iter().filter(|p| p.qualifier != ParamQual::Out).collect();

        for p in &f.params {
            self.locals.insert(p.name.clone(), p.ty);
        }

        let struct_name = format!("{}Output", f.name);

        // Output struct for functions with `out` params.
        if !out_params.is_empty() {
            let _ = writeln!(self.out, "struct {struct_name} {{");
            for (i, p) in out_params.iter().enumerate() {
                let _ = writeln!(self.out, "    @location({i}) {}: {},", sanitize_ident(&p.name), wgsl_type(p.ty));
            }
            let _ = writeln!(self.out, "}}\n");
        }

        // HLSL lets you assign to `in` parameters, but WGSL parameters are
        // immutable and can't be shadowed by a same-named local. So a written-to
        // parameter is renamed `<name>_param` and copied into a mutable local.
        let mut mutated = std::collections::HashSet::new();
        collect_mutated(&f.body, &mut mutated);

        // Signature.
        let params_s = in_params
            .iter()
            .map(|p| {
                let base = sanitize_ident(&p.name);
                let pname = if mutated.contains(&p.name) { format!("{base}_param") } else { base };
                format!("{}: {}", pname, wgsl_type(p.ty))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let ret_s = if !out_params.is_empty() {
            format!(" -> {struct_name}")
        } else if f.ret != Type::Void {
            format!(" -> {}", wgsl_type(f.ret))
        } else {
            String::new()
        };
        let _ = writeln!(self.out, "fn {}({}){} {{", sanitize_ident(&f.name), params_s, ret_s);

        // `out` params become mutable locals initialized to zero.
        for p in &out_params {
            let _ = writeln!(self.out, "    var {}: {} = {};", sanitize_ident(&p.name), wgsl_type(p.ty), zero_value(p.ty));
        }

        // Mutable local copies of written-to parameters.
        for p in &in_params {
            if mutated.contains(&p.name) {
                let _ = writeln!(self.out, "    var {0} = {0}_param;", sanitize_ident(&p.name));
            }
        }

        let return_expr = if out_params.is_empty() {
            None
        } else {
            let fields = out_params.iter().map(|p| sanitize_ident(&p.name)).collect::<Vec<_>>().join(", ");
            Some(format!("{struct_name}({fields})"))
        };

        for s in &f.body {
            self.stmt(s, 1, return_expr.as_deref());
        }

        // Implicit return for the out-param struct at fall-off.
        if let Some(re) = &return_expr {
            let _ = writeln!(self.out, "    return {re};");
        }

        let _ = writeln!(self.out, "}}\n");
    }

    // ------------------------------------------------------- statements ------

    fn stmt(&mut self, s: &Stmt, indent: usize, ret: Option<&str>) {
        let pad = "    ".repeat(indent);
        match s {
            Stmt::Decl { ty, name, init } => {
                self.locals.insert(name.clone(), *ty);
                let name = sanitize_ident(name);
                match init {
                    Some(e) => {
                        let v = self.emit_broadcast(e, *ty);
                        let _ = writeln!(self.out, "{pad}var {name}: {} = {v};", wgsl_type(*ty));
                    }
                    None => {
                        let _ = writeln!(self.out, "{pad}var {name}: {} = {};", wgsl_type(*ty), zero_value(*ty));
                    }
                }
            }
            Stmt::Expr(e) => self.emit_expr_stmt(e, indent),
            Stmt::Return(value) => match (value, ret) {
                // Inside an out-param function, `return` yields the struct.
                (_, Some(re)) => {
                    let _ = writeln!(self.out, "{pad}return {re};");
                }
                (Some(e), None) => {
                    let _ = writeln!(self.out, "{pad}return {};", self.emit_return_value(e));
                }
                (None, None) => {
                    let _ = writeln!(self.out, "{pad}return;");
                }
            },
            Stmt::Block(stmts) => {
                let _ = writeln!(self.out, "{pad}{{");
                for st in stmts {
                    self.stmt(st, indent + 1, ret);
                }
                let _ = writeln!(self.out, "{pad}}}");
            }
            Stmt::If(cond, then, els) => {
                let _ = writeln!(self.out, "{pad}if ({}) {{", self.expr(cond, Type::Bool));
                self.stmt_as_block(then, indent + 1, ret);
                if let Some(e) = els {
                    let _ = writeln!(self.out, "{pad}}} else {{");
                    self.stmt_as_block(e, indent + 1, ret);
                }
                let _ = writeln!(self.out, "{pad}}}");
            }
            Stmt::While(cond, body) => {
                let _ = writeln!(self.out, "{pad}while ({}) {{", self.expr(cond, Type::Bool));
                self.stmt_as_block(body, indent + 1, ret);
                let _ = writeln!(self.out, "{pad}}}");
            }
            Stmt::For(init, cond, update, body) => {
                // Lower to an initializer + while loop to avoid WGSL for-header limits.
                let _ = writeln!(self.out, "{pad}{{");
                if let Some(i) = init {
                    self.stmt(i, indent + 1, ret);
                }
                let cond_s = match cond {
                    Some(c) => self.expr(c, Type::Bool),
                    None => "true".to_string(),
                };
                let _ = writeln!(self.out, "{pad}    while ({cond_s}) {{");
                self.stmt_as_block(body, indent + 2, ret);
                if let Some(u) = update {
                    self.emit_expr_stmt(u, indent + 2);
                }
                let _ = writeln!(self.out, "{pad}    }}");
                let _ = writeln!(self.out, "{pad}}}");
            }
        }
    }

    /// Emit a statement, ensuring its contents are at the given indent (a bare
    /// block's inner statements are spliced in without extra braces).
    fn stmt_as_block(&mut self, s: &Stmt, indent: usize, ret: Option<&str>) {
        if let Stmt::Block(stmts) = s {
            for st in stmts {
                self.stmt(st, indent, ret);
            }
        } else {
            self.stmt(s, indent, ret);
        }
    }

    /// Emit a returned expression, coercing it to the function's declared return
    /// type only where needed. The single coercion implemented is numeric ->
    /// bool: HLSL lets a `bool`-returning function `return` a numeric mask (e.g.
    /// `(a > b) * (c < d)`), which is true iff nonzero. A return already inferred
    /// as bool, and every non-bool return, keep their existing emission.
    fn emit_return_value(&self, e: &Expr) -> String {
        if self.current_ret == Type::Bool {
            match self.infer(e) {
                Type::Bool => return self.expr(e, Type::Bool),
                Type::Float => return format!("({}) != 0.0", self.expr(e, Type::Float)),
                Type::Int => return format!("({}) != 0", self.expr(e, Type::Int)),
                // Ambiguous (a vector or other type from a `bool` function): leave
                // it as-is rather than guess a conversion.
                _ => {}
            }
        }
        self.expr(e, Type::Float)
    }

    /// Statement-position expression: handles increment/decrement and the
    /// swizzle-lvalue assignment expansion that WGSL requires.
    fn emit_expr_stmt(&mut self, e: &Expr, indent: usize) {
        let pad = "    ".repeat(indent);
        match e {
            // HLSL `x++`/`x--` work on floats; WGSL `++`/`--` are int-only, so
            // lower to `x = x + 1` with a literal matching the operand type.
            Expr::PostInc(x) | Expr::PreInc(x) => {
                let ty = self.infer(x);
                let target = self.expr(x, scalar_of(ty));
                let one = broadcast_scalar_literal(ty, if scalar_of(ty) == Type::Int { "1" } else { "1.0" });
                let _ = writeln!(self.out, "{pad}{target} = {target} + {one};");
            }
            Expr::PostDec(x) | Expr::PreDec(x) => {
                let ty = self.infer(x);
                let target = self.expr(x, scalar_of(ty));
                let one = broadcast_scalar_literal(ty, if scalar_of(ty) == Type::Int { "1" } else { "1.0" });
                let _ = writeln!(self.out, "{pad}{target} = {target} - {one};");
            }
            Expr::Assign(op, lhs, rhs) => self.emit_assign(*op, lhs, rhs, indent),
            other => {
                let _ = writeln!(self.out, "{pad}{};", self.expr(other, Type::Float));
            }
        }
    }

    fn emit_assign(&mut self, op: AssignOp, lhs: &Expr, rhs: &Expr, indent: usize) {
        let pad = "    ".repeat(indent);
        let target_ty = self.infer(lhs);
        let op_s = match op {
            AssignOp::Assign => "=",
            AssignOp::Add => "+=",
            AssignOp::Sub => "-=",
            AssignOp::Mul => "*=",
            AssignOp::Div => "/=",
            AssignOp::Mod => "%=",
        };

        // Multi-component swizzle on the left needs component-wise assignment.
        if let Expr::Member(base, field) = lhs {
            if is_swizzle(field) && field.len() > 1 {
                let base_s = self.expr(base, Type::Float);
                let rhs_s = self.emit_broadcast(rhs, target_ty);
                let tmp = self.fresh_temp();
                let _ = writeln!(self.out, "{pad}{{");
                let _ = writeln!(self.out, "{pad}    let {tmp} = {rhs_s};");
                for (i, c) in field.chars().enumerate() {
                    let _ = writeln!(self.out, "{pad}    {base_s}.{c} {op_s} {tmp}[{i}];");
                }
                let _ = writeln!(self.out, "{pad}}}");
                return;
            }
        }

        let lhs_s = self.expr(lhs, Type::Float);
        let rhs_s = self.emit_broadcast(rhs, target_ty);
        let _ = writeln!(self.out, "{pad}{lhs_s} {op_s} {rhs_s};");
    }

    // ------------------------------------------------------ expressions ------

    /// Emit `e` as a numeric scalar of type `want`, inserting an explicit cast
    /// when HLSL would implicitly convert and WGSL won't: a bool (comparison /
    /// logical result) used numerically becomes `f32`/`i32`, and a scalar `int`
    /// reaching a `float` context becomes `f32` (HLSL int->float promotion, e.g.
    /// `floatVar <= intVar`). Returns the *effective* type (the numeric scalar
    /// after a cast, else the original inferred type) and the emitted text. Int
    /// stays int in int contexts (`want == Int`), and integer literals — already
    /// emitted as floats by `expr` when a float is wanted — are left alone.
    fn emit_numeric_coerced(&self, e: &Expr, want: Type) -> (Type, String) {
        let et = self.infer(e);
        let s = self.expr(e, want);
        match (et, want) {
            (Type::Bool, Type::Float) => (Type::Float, format!("f32({s})")),
            (Type::Bool, Type::Int) => (Type::Int, format!("i32({s})")),
            (Type::Int, Type::Float) if !matches!(e, Expr::IntLit(_)) => (Type::Float, format!("f32({s})")),
            _ => (et, s),
        }
    }

    /// Emit `e` for a numeric context (e.g. a `floatN(…)` constructor argument),
    /// coercing bool/int to `f32`/`i32` as HLSL does.
    fn emit_numeric(&self, e: &Expr, want: Type) -> String {
        self.emit_numeric_coerced(e, want).1
    }

    /// Emit `e`, coercing it to `target`: broadcast a scalar up to a vector, or
    /// truncate a wider vector down (HLSL implicitly drops trailing components,
    /// e.g. `float3 v = tex2D(...)` keeps `.xyz`).
    fn emit_broadcast(&self, e: &Expr, target: Type) -> String {
        // HLSL uses comparison/logical (bool) results numerically; WGSL needs an
        // explicit cast when a bool reaches a float/int context. After the cast
        // the value is a numeric scalar, so the broadcast/truncate logic below
        // proceeds as usual.
        let (et, s) = self.emit_numeric_coerced(e, scalar_of(target));
        // A bool/int *vector* reaching a numeric vector of a different scalar
        // kind: WGSL's component-wise conversion (`vec3<f32>(vec3<bool>)` -> 1/0,
        // `vec3<f32>(vec3<i32>)` -> int->float). Identity (int->int, float->...)
        // is excluded.
        if et.vector_len().is_some()
            && et.vector_len() == target.vector_len()
            && matches!(
                (scalar_of(et), scalar_of(target)),
                (Type::Bool, Type::Float) | (Type::Bool, Type::Int) | (Type::Int, Type::Float)
            )
        {
            return format!("{}({})", wgsl_type(target), s);
        }
        let target_w = target.vector_len().map(usize::from).unwrap_or(target.is_scalar() as usize);
        let expr_w = et.vector_len().map(usize::from).unwrap_or(et.is_scalar() as usize);
        if target.vector_len().is_some() && et.is_scalar() {
            // broadcast a scalar across the target vector
            format!("{}({})", wgsl_type(target), s)
        } else if target_w >= 1 && expr_w > target_w {
            // truncate a wider source to the target width (scalar target -> `.x`)
            let swizzle = &"xyzw"[..target_w];
            format!("({s}).{swizzle}")
        } else {
            s
        }
    }

    /// `want` is the preferred scalar type used only to format numeric literals.
    fn expr(&self, e: &Expr, want: Type) -> String {
        match e {
            Expr::IntLit(v) => {
                if want == Type::Int {
                    format!("{v}")
                } else {
                    format!("{v}.0")
                }
            }
            Expr::FloatLit(v) => fmt_float(*v),
            Expr::BoolLit(b) => b.to_string(),
            Expr::Ident(name) => sanitize_ident(name),
            // Unary minus on a bool (a comparison/logical result) needs the bool
            // coerced to a number *first* (HLSL `-(a < b)` = `-(0/1)`): scalar ->
            // `-(f32(...))`, bool vector -> `-(vecN<f32>(...))`.
            Expr::Unary(UnOp::Neg, x) => {
                let xt = self.infer(x);
                if scalar_of(xt) == Type::Bool {
                    let inner = self.expr(x, Type::Float);
                    let coerced = match xt.vector_len() {
                        Some(n) => format!("{}({inner})", wgsl_type(vec_of(Type::Float, n))),
                        None => format!("f32({inner})"),
                    };
                    format!("-({coerced})")
                } else {
                    format!("-({})", self.expr(x, want))
                }
            }
            Expr::Unary(op, x) => {
                let s = match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "!",
                    UnOp::BitNot => "~",
                };
                let inner_want = if *op == UnOp::Not { Type::Bool } else { want };
                format!("{s}({})", self.expr(x, inner_want))
            }
            Expr::PreInc(x) | Expr::PostInc(x) => self.expr(x, want), // statements handle ++
            Expr::PreDec(x) | Expr::PostDec(x) => self.expr(x, want),
            Expr::Binary(op, a, b) => self.emit_binary(*op, a, b),
            Expr::Assign(_, lhs, _) => self.expr(lhs, want), // assignments are statements
            Expr::Ternary(c, t, f) => {
                // WGSL: select(false_value, true_value, condition).
                let ty = self.infer(t);
                format!(
                    "select({}, {}, {})",
                    self.emit_broadcast(f, ty),
                    self.expr(t, scalar_of(ty)),
                    self.expr(c, Type::Bool)
                )
            }
            Expr::Member(base, field) => {
                let bt = self.infer(base);
                // HLSL allows swizzles on scalars (`s.x` -> s, `s.xxx` -> broadcast);
                // WGSL has no scalar field accessor, so rewrite them.
                if bt.is_scalar() && is_swizzle(field) {
                    let s = self.expr(base, scalar_of(bt));
                    if field.len() == 1 {
                        s
                    } else {
                        format!("{}({})", wgsl_type(vec_of(bt, field.len() as u8)), s)
                    }
                } else {
                    format!("{}.{}", self.expr(base, Type::Float), field)
                }
            }
            Expr::Index(base, idx) => {
                format!("{}[{}]", self.expr(base, Type::Float), self.expr(idx, Type::Int))
            }
            Expr::Construct(ty, args) => {
                // HLSL `float3(x > 0.5)` etc.: a bool argument becomes 1.0/0.0.
                let scalar = scalar_of(*ty);
                let parts = args.iter().map(|a| self.emit_numeric(a, scalar)).collect::<Vec<_>>().join(", ");
                format!("{}({})", wgsl_type(*ty), parts)
            }
            Expr::Call(name, args) => self.emit_call(name, args),
        }
    }

    fn emit_binary(&self, op: BinOp, a: &Expr, b: &Expr) -> String {
        // Logical operators: bool operands, `&&` / `||`.
        if matches!(op, BinOp::And | BinOp::Or) {
            let o = if op == BinOp::And { "&&" } else { "||" };
            return format!("({} {} {})", self.expr(a, Type::Bool), o, self.expr(b, Type::Bool));
        }

        let ta = self.infer(a);
        let tb = self.infer(b);

        // Comparisons -> bool(vec). Broadcast operands to a common arithmetic type.
        if matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge) {
            let common = arith_common(ta, tb);
            let o = bin_op_str(op);
            return format!("({} {} {})", self.emit_broadcast(a, common), o, self.emit_broadcast(b, common));
        }

        // Numeric arithmetic on bools (HLSL `(a > b) * (c > d)`) is float math
        // (`&&`/`||` and comparisons returned early; bitwise/shift stay integral).
        let common = arith_binary_type(op, ta, tb);
        let o = bin_op_str(op);
        format!("({} {} {})", self.emit_broadcast(a, common), o, self.emit_broadcast(b, common))
    }

    fn emit_call(&self, name: &str, args: &[Expr]) -> String {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            // Texture sampling. Binding convention: `<s>` texture, `<s>_sampler`.
            // 2D sampling: HLSL `tex2D` uses only the first two coordinate
            // components, so truncate a wider coordinate (e.g. `GetBlur1(uv)`
            // returns float3) to a `vec2` for WGSL. A vec2 is left unchanged.
            "tex2d" | "tex2dlod" | "tex2dbias" => {
                if args.len() >= 2 {
                    let s = self.expr(&args[0], Type::Float);
                    let uv = self.emit_broadcast(&args[1], Type::Float2);
                    return format!("textureSample({s}, {s}_sampler, {uv})");
                }
            }
            // 3D sampling keeps its vec3 coordinate unchanged.
            "tex3d" => {
                if args.len() >= 2 {
                    let s = self.expr(&args[0], Type::Float);
                    let uv = self.expr(&args[1], Type::Float);
                    return format!("textureSample({s}, {s}_sampler, {uv})");
                }
            }
            "lerp" => {
                if args.len() == 3 {
                    // `mix` needs all three operands to share one type; broadcast
                    // each to the widest (e.g. `lerp(scalar, scalar, vec3)`).
                    let common = args.iter().map(|a| self.infer(a)).fold(Type::Float, arith_common);
                    let parts =
                        args.iter().map(|a| self.emit_broadcast(a, common)).collect::<Vec<_>>().join(", ");
                    return format!("mix({parts})");
                }
            }
            "saturate" => {
                if args.len() == 1 {
                    let ty = self.infer(&args[0]);
                    let x = self.expr(&args[0], scalar_of(ty));
                    let zero = broadcast_scalar_literal(ty, "0.0");
                    let one = broadcast_scalar_literal(ty, "1.0");
                    return format!("clamp({x}, {zero}, {one})");
                }
            }
            "frac" => return self.simple_call("fract", args),
            "rsqrt" => return self.simple_call("inverseSqrt", args),
            "ddx" => return self.simple_call("dpdx", args),
            "ddy" => return self.simple_call("dpdy", args),
            "atan2" => return self.simple_call("atan2", args),
            "fmod" => {
                if args.len() == 2 {
                    let common = arith_common(self.infer(&args[0]), self.infer(&args[1]));
                    return format!(
                        "({} % {})",
                        self.emit_broadcast(&args[0], common),
                        self.emit_broadcast(&args[1], common)
                    );
                }
            }
            "mul" => {
                if args.len() == 2 {
                    return format!(
                        "({} * {})",
                        self.expr(&args[0], Type::Float),
                        self.expr(&args[1], Type::Float)
                    );
                }
            }
            // Intrinsics whose arguments must share one type. Component-wise ones
            // (`pow`/`min`/…) and reductive ones (`dot`/`distance` → scalar) alike
            // need every argument coerced to a common vector width: scalars
            // broadcast up, and a wider operand truncates to match the narrower
            // (HLSL's rule, e.g. `dot(float4, float3)` operates on the first 3).
            "pow" | "min" | "max" | "step" | "clamp" | "atan" | "dot" | "cross" | "distance" | "reflect"
            | "smoothstep" => {
                let common = args.iter().map(|a| self.infer(a)).fold(Type::Float, arith_common);
                if common.vector_len().is_some() {
                    let parts =
                        args.iter().map(|a| self.emit_broadcast(a, common)).collect::<Vec<_>>().join(", ");
                    return format!("{lower}({parts})");
                }
            }
            _ => {}
        }
        // A user-defined helper: coerce each argument to its declared parameter
        // type (HLSL broadcasts e.g. a scalar `0.5` to a `float2` param).
        if let Some((_, params)) = self.functions.get(name) {
            if params.len() == args.len() {
                let parts = args
                    .iter()
                    .zip(params)
                    .map(|(a, &pty)| self.emit_broadcast(a, pty))
                    .collect::<Vec<_>>()
                    .join(", ");
                return format!("{}({parts})", sanitize_ident(name));
            }
        }
        // Default: pass through with float-formatted literals.
        self.simple_call(&lower, args)
    }

    fn simple_call(&self, wgsl_name: &str, args: &[Expr]) -> String {
        let parts = args.iter().map(|a| self.expr(a, Type::Float)).collect::<Vec<_>>().join(", ");
        format!("{wgsl_name}({parts})")
    }

    // -------------------------------------------------- type inference -------

    fn infer(&self, e: &Expr) -> Type {
        match e {
            Expr::IntLit(_) => Type::Int,
            Expr::FloatLit(_) => Type::Float,
            Expr::BoolLit(_) => Type::Bool,
            Expr::Ident(name) => self
                .locals
                .get(name)
                .or_else(|| self.globals.get(name))
                .copied()
                .unwrap_or(Type::Float),
            Expr::Unary(UnOp::Not, _) => Type::Bool,
            // Unary minus on a bool coerces to a number (see codegen), so its
            // inferred type is numeric — otherwise the surrounding context would
            // wrap it in a redundant/invalid second cast.
            Expr::Unary(UnOp::Neg, x) => {
                let xt = self.infer(x);
                if scalar_of(xt) == Type::Bool {
                    xt.vector_len().map_or(Type::Float, |n| vec_of(Type::Float, n))
                } else {
                    xt
                }
            }
            Expr::Unary(_, x) => self.infer(x),
            Expr::PreInc(x) | Expr::PostInc(x) | Expr::PreDec(x) | Expr::PostDec(x) => self.infer(x),
            Expr::Binary(op, a, b) => match op {
                // Logical operators take/return scalar bool.
                BinOp::And | BinOp::Or => Type::Bool,
                // Comparisons are component-wise: a vector comparison yields a
                // bool *vector* (`vec3 > vec3` -> `vec3<bool>`), which matters
                // for whether a `f32()` (scalar) or `vecN<f32>()` (vector)
                // conversion is needed when it reaches a numeric context.
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                    match arith_common(self.infer(a), self.infer(b)).vector_len() {
                        Some(n) => vec_of(Type::Bool, n),
                        None => Type::Bool,
                    }
                }
                // Arithmetic: bool operands promote to float (matches emission).
                _ => arith_binary_type(*op, self.infer(a), self.infer(b)),
            },
            Expr::Assign(_, lhs, _) => self.infer(lhs),
            Expr::Ternary(_, t, _) => self.infer(t),
            Expr::Member(base, field) => member_type(self.infer(base), field),
            Expr::Index(base, _) => index_type(self.infer(base)),
            Expr::Construct(ty, _) => *ty,
            Expr::Call(name, args) => self.call_type(name, args),
        }
    }

    fn call_type(&self, name: &str, args: &[Expr]) -> Type {
        // A user-defined helper's declared return type wins over the heuristic.
        if let Some((ret, _)) = self.functions.get(name) {
            return *ret;
        }
        match name.to_ascii_lowercase().as_str() {
            "dot" | "length" | "distance" | "determinant" => Type::Float,
            "tex2d" | "tex3d" | "tex2dlod" | "tex2dbias" => Type::Float4,
            "cross" => Type::Float3,
            // Component-wise intrinsics: the result is the common type across
            // *all* arguments, not just the first — e.g. `lerp(0, vec3, vec3)`
            // returns vec3 (the int `0` broadcasts), while `min(intA, intB)`
            // stays int. `reduce` seeds from the first arg so a vector arg wins
            // over a leading scalar without forcing the whole thing to float.
            "smoothstep" | "lerp" | "min" | "max" | "clamp" | "step" | "pow" | "reflect" | "atan" | "fmod" => {
                args.iter().map(|a| self.infer(a)).reduce(arith_common).unwrap_or(Type::Float)
            }
            "any" | "all" => Type::Bool,
            "mul" => args.iter().map(|a| self.infer(a)).find(|t| t.vector_len().is_some()).unwrap_or(Type::Float4),
            _ => args.first().map(|a| self.infer(a)).unwrap_or(Type::Float),
        }
    }
}

// ----------------------------------------------------------- helpers ---------

/// Collect the root identifiers that are assigned to anywhere in `stmts`
/// (the targets of `=`/compound assignment and `++`/`--`).
fn collect_mutated(stmts: &[Stmt], out: &mut std::collections::HashSet<String>) {
    for s in stmts {
        collect_mutated_stmt(s, out);
    }
}

fn collect_mutated_stmt(s: &Stmt, out: &mut std::collections::HashSet<String>) {
    match s {
        Stmt::Decl { init: Some(e), .. } => collect_mutated_expr(e, out),
        Stmt::Decl { init: None, .. } => {}
        Stmt::Expr(e) | Stmt::Return(Some(e)) => collect_mutated_expr(e, out),
        Stmt::Return(None) => {}
        Stmt::Block(b) => collect_mutated(b, out),
        Stmt::If(c, t, e) => {
            collect_mutated_expr(c, out);
            collect_mutated_stmt(t, out);
            if let Some(e) = e {
                collect_mutated_stmt(e, out);
            }
        }
        Stmt::While(c, b) => {
            collect_mutated_expr(c, out);
            collect_mutated_stmt(b, out);
        }
        Stmt::For(init, cond, update, body) => {
            if let Some(i) = init {
                collect_mutated_stmt(i, out);
            }
            if let Some(c) = cond {
                collect_mutated_expr(c, out);
            }
            if let Some(u) = update {
                collect_mutated_expr(u, out);
            }
            collect_mutated_stmt(body, out);
        }
    }
}

/// WGSL keywords + reserved words that are also valid HLSL identifiers, so a
/// preset variable/function could collide with one (e.g. `mod`, `filter`,
/// `move`). Generated names (`_cN`, `ret`, `sampler_main`, …) never match.
const WGSL_RESERVED: &[&str] = &[
    "NULL", "Self", "abstract", "active", "alias", "as", "asm", "async", "atomic", "auto", "await",
    "become", "binding_array", "bitcast", "cast", "catch", "class", "co_await", "co_return",
    "co_yield", "coherent", "common", "compile", "concept", "const_cast", "consteval", "constexpr",
    "constinit", "crate", "debugger", "decltype", "delete", "demote", "do", "dynamic_cast", "enum",
    "explicit", "export", "extends", "extern", "external", "fallthrough", "filter", "final",
    "finally", "friend", "from", "get", "goto", "groupshared", "highp", "impl", "implements",
    "import", "inline", "instanceof", "interface", "layout", "lowp", "macro", "match", "mediump",
    "meta", "mod", "module", "move", "mut", "mutable", "namespace", "new", "nil", "noexcept",
    "noinline", "null", "nullptr", "of", "operator", "override", "package", "packoffset",
    "partition", "pass", "patch", "pixelfragment", "precise", "precision", "premerge", "priv",
    "protected", "ptr", "pub", "public", "readonly", "ref", "regardless", "register",
    "reinterpret_cast", "require", "resource", "restrict", "self", "set", "shared", "signed",
    "sizeof", "smooth", "snorm", "static_assert", "static_cast", "std", "subroutine", "super",
    "target", "template", "this", "thread_local", "throw", "trait", "try", "type", "typedef",
    "typeid", "typename", "typeof", "union", "unless", "unorm", "unsafe", "unsized", "use", "using",
    "varying", "virtual", "volatile", "where", "with", "writeonly", "yield",
];

/// Rename an identifier that collides with a WGSL reserved word (`mod` ->
/// `mod_pm`). Idempotent: the sanitized form is itself not reserved, so applying
/// it again is a no-op. Used at every WGSL identifier *emission* site;
/// type-inference maps keep the original names.
fn sanitize_ident(name: &str) -> String {
    if WGSL_RESERVED.contains(&name) {
        format!("{name}_pm")
    } else {
        name.to_string()
    }
}

fn collect_mutated_expr(e: &Expr, out: &mut std::collections::HashSet<String>) {
    match e {
        Expr::Assign(_, target, value) => {
            if let Some(name) = root_ident(target) {
                out.insert(name);
            }
            collect_mutated_expr(value, out);
        }
        Expr::PreInc(x) | Expr::PostInc(x) | Expr::PreDec(x) | Expr::PostDec(x) => {
            if let Some(name) = root_ident(x) {
                out.insert(name);
            }
        }
        Expr::Unary(_, x) => collect_mutated_expr(x, out),
        Expr::Binary(_, a, b) => {
            collect_mutated_expr(a, out);
            collect_mutated_expr(b, out);
        }
        Expr::Ternary(c, t, f) => {
            collect_mutated_expr(c, out);
            collect_mutated_expr(t, out);
            collect_mutated_expr(f, out);
        }
        Expr::Member(b, _) => collect_mutated_expr(b, out),
        Expr::Index(b, i) => {
            collect_mutated_expr(b, out);
            collect_mutated_expr(i, out);
        }
        Expr::Call(_, args) | Expr::Construct(_, args) => {
            for a in args {
                collect_mutated_expr(a, out);
            }
        }
        Expr::FloatLit(_) | Expr::IntLit(_) | Expr::BoolLit(_) | Expr::Ident(_) => {}
    }
}

/// The root variable name of an l-value expression (`a.b[c].d` -> `a`).
fn root_ident(e: &Expr) -> Option<String> {
    match e {
        Expr::Ident(name) => Some(name.clone()),
        Expr::Member(b, _) | Expr::Index(b, _) => root_ident(b),
        _ => None,
    }
}

fn wgsl_type(ty: Type) -> &'static str {
    use Type::*;
    match ty {
        Void => "()",
        Float => "f32",
        Int => "i32",
        Bool => "bool",
        Float2 => "vec2<f32>",
        Float3 => "vec3<f32>",
        Float4 => "vec4<f32>",
        Int2 => "vec2<i32>",
        Int3 => "vec3<i32>",
        Int4 => "vec4<i32>",
        Bool2 => "vec2<bool>",
        Bool3 => "vec3<bool>",
        Bool4 => "vec4<bool>",
        Mat2 => "mat2x2<f32>",
        Mat3 => "mat3x3<f32>",
        Mat4 => "mat4x4<f32>",
        // HLSL floatRxC (rows x cols) -> WGSL matCxR (cols x rows).
        Mat4x3 => "mat3x4<f32>",
        Mat3x4 => "mat4x3<f32>",
        Sampler2D | Sampler3D => "texture_2d<f32>",
    }
}

fn zero_value(ty: Type) -> String {
    match ty {
        Type::Float => "0.0".into(),
        Type::Int => "0".into(),
        Type::Bool => "false".into(),
        _ => format!("{}()", wgsl_type(ty)),
    }
}

/// The scalar component type of `ty` (vectors -> their base scalar).
fn scalar_of(ty: Type) -> Type {
    use Type::*;
    match ty {
        Float2 | Float3 | Float4 | Mat2 | Mat3 | Mat4 | Mat4x3 | Mat3x4 => Float,
        Int2 | Int3 | Int4 => Int,
        Bool2 | Bool3 | Bool4 => Bool,
        other => other,
    }
}

fn vec_of(scalar: Type, n: u8) -> Type {
    use Type::*;
    match (scalar, n) {
        (Float, 2) => Float2,
        (Float, 3) => Float3,
        (Float, 4) => Float4,
        (Int, 2) => Int2,
        (Int, 3) => Int3,
        (Int, 4) => Int4,
        (Bool, 2) => Bool2,
        (Bool, 3) => Bool3,
        (Bool, 4) => Bool4,
        _ => scalar,
    }
}

/// Common arithmetic result type of two operands (float wins over int). When
/// both are vectors of different widths, HLSL truncates the wider to the
/// narrower (`float4 * float2` operates on the first two lanes); a scalar
/// operand broadcasts up to the vector width.
fn arith_common(a: Type, b: Type) -> Type {
    let scalar = if scalar_of(a) == Type::Float || scalar_of(b) == Type::Float {
        Type::Float
    } else if scalar_of(a) == Type::Int || scalar_of(b) == Type::Int {
        Type::Int
    } else {
        Type::Bool
    };
    let n = match (a.vector_len(), b.vector_len()) {
        (Some(na), Some(nb)) => Some(na.min(nb)),
        (Some(na), None) => Some(na),
        (None, Some(nb)) => Some(nb),
        (None, None) => None,
    };
    match n {
        Some(n) => vec_of(scalar, n),
        None => scalar,
    }
}

/// Result type of an arithmetic binary op, with bool promoted to float for the
/// numeric ops (`(a > b) * (c < d)` is float math). Shared by `emit_binary` and
/// `infer` so the emitted code and its inferred type agree.
fn arith_binary_type(op: BinOp, ta: Type, tb: Type) -> Type {
    let common = arith_common(ta, tb);
    if matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod)
        && matches!(scalar_of(common), Type::Bool)
    {
        common.vector_len().map_or(Type::Float, |n| vec_of(Type::Float, n))
    } else {
        common
    }
}

fn member_type(base: Type, field: &str) -> Type {
    if (base.vector_len().is_some() || base.is_scalar()) && is_swizzle(field) {
        let scalar = scalar_of(base);
        if field.len() == 1 {
            scalar
        } else {
            vec_of(scalar, field.len() as u8)
        }
    } else {
        // Matrix column access or unknown struct field: best-effort.
        scalar_of(base)
    }
}

fn index_type(base: Type) -> Type {
    match base {
        Type::Mat2 => Type::Float2,
        Type::Mat3 => Type::Float3,
        Type::Mat4 => Type::Float4,
        other => scalar_of(other),
    }
}

fn is_swizzle(field: &str) -> bool {
    !field.is_empty() && field.len() <= 4 && field.chars().all(|c| "xyzwrgba".contains(c))
}

fn bin_op_str(op: BinOp) -> &'static str {
    use BinOp::*;
    match op {
        Add => "+",
        Sub => "-",
        Mul => "*",
        Div => "/",
        Mod => "%",
        Eq => "==",
        Ne => "!=",
        Lt => "<",
        Gt => ">",
        Le => "<=",
        Ge => ">=",
        BitAnd => "&",
        BitOr => "|",
        BitXor => "^",
        Shl => "<<",
        Shr => ">>",
        And => "&&",
        Or => "||",
    }
}

fn broadcast_scalar_literal(ty: Type, lit: &str) -> String {
    if ty.vector_len().is_some() {
        format!("{}({lit})", wgsl_type(ty))
    } else {
        lit.to_string()
    }
}

fn fmt_float(v: f64) -> String {
    if !v.is_finite() {
        return if v > 0.0 { "3.4e38".into() } else if v < 0.0 { "-3.4e38".into() } else { "0.0".into() };
    }
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}
