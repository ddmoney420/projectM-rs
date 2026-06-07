//! Tree-walking interpreter for compiled [`Expr`] programs.
//!
//! Semantics mirror ns-eel / projectm-eval where they differ from plain IEEE
//! math, because preset authors rely on the quirks:
//!   * division / modulo by zero yields `0.0` (never NaN/inf),
//!   * `==` / `!=` compare with an epsilon of `1e-5`,
//!   * "truthiness" for `if`/`while`/`&&`/`||`/`!` is `|v| > 1e-5`.

use crate::ast::{BinOp, Expr, UnOp};
use std::collections::HashMap;
use std::fmt;

/// ns-eel comparison / truthiness epsilon.
const EPS: f64 = 1e-5;

/// Hard cap on `loop()` / `while()` iterations to keep a runaway preset from
/// hanging the host. ns-eel applies a similar safety limit.
const MAX_ITERS: u64 = 1 << 21;

fn truthy(v: f64) -> bool {
    v.abs() > EPS
}

#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    UnknownFunction(String),
    Arity { name: String, expected: &'static str, got: usize },
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::UnknownFunction(n) => write!(f, "unknown function '{n}'"),
            EvalError::Arity { name, expected, got } => {
                write!(f, "'{name}' expects {expected} argument(s), got {got}")
            }
        }
    }
}

impl std::error::Error for EvalError {}

/// Execution state: variables, per-instance and global memory, and the RNG.
///
/// Variables are addressed by lowercased name. Memory is sparse — unset cells
/// read as `0.0`, matching ns-eel's zero-initialized megabuf.
#[derive(Debug, Clone)]
pub struct Context {
    vars: HashMap<String, f64>,
    megabuf: HashMap<i64, f64>,
    gmegabuf: HashMap<i64, f64>,
    rng: u64,
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl Context {
    pub fn new() -> Self {
        let mut ctx = Context {
            vars: HashMap::new(),
            megabuf: HashMap::new(),
            gmegabuf: HashMap::new(),
            rng: 0x9E37_79B9_7F4A_7C15,
        };
        // Named constants exposed as read-able (and over-writable) variables.
        ctx.vars.insert("pi".into(), std::f64::consts::PI);
        ctx.vars.insert("e".into(), std::f64::consts::E);
        ctx.vars.insert("phi".into(), 1.618_033_988_749_895);
        ctx
    }

    /// Seed the `rand()` generator deterministically (useful for tests).
    pub fn seed(&mut self, seed: u64) {
        self.rng = seed | 1;
    }

    pub fn set(&mut self, name: &str, value: f64) {
        self.vars.insert(name.to_ascii_lowercase(), value);
    }

    pub fn get(&self, name: &str) -> f64 {
        self.vars.get(&name.to_ascii_lowercase()).copied().unwrap_or(0.0)
    }

    fn next_rand(&mut self) -> f64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        // Top 53 bits → uniform f64 in [0, 1).
        (v >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Evaluate a program against this context, returning the last value.
    pub fn run(&mut self, program: &Expr) -> Result<f64, EvalError> {
        self.eval(program)
    }

    fn eval(&mut self, e: &Expr) -> Result<f64, EvalError> {
        match e {
            Expr::Num(n) => Ok(*n),
            Expr::Var(name) => Ok(self.vars.get(name).copied().unwrap_or(0.0)),
            Expr::Mem { base, offset } => {
                let addr = self.mem_addr(base, offset.as_deref())?;
                Ok(self.megabuf.get(&addr).copied().unwrap_or(0.0))
            }
            Expr::Unary(op, x) => {
                let v = self.eval(x)?;
                Ok(match op {
                    UnOp::Neg => -v,
                    UnOp::Not => bool_f(!truthy(v)),
                })
            }
            Expr::Binary(op, a, b) => self.eval_binary(*op, a, b),
            Expr::Assign { target, op, value } => self.eval_assign(target, *op, value),
            Expr::Block(stmts) => {
                let mut last = 0.0;
                for s in stmts {
                    last = self.eval(s)?;
                }
                Ok(last)
            }
            Expr::Call(name, args) => self.eval_call(name, args),
        }
    }

    fn eval_binary(&mut self, op: BinOp, a: &Expr, b: &Expr) -> Result<f64, EvalError> {
        // Short-circuit logical operators.
        match op {
            BinOp::And => {
                return Ok(bool_f(truthy(self.eval(a)?) && truthy(self.eval(b)?)));
            }
            BinOp::Or => {
                let av = self.eval(a)?;
                if truthy(av) {
                    // Still must not evaluate b — but eval b only matters for
                    // side effects; ns-eel short-circuits, so we do too.
                    return Ok(1.0);
                }
                return Ok(bool_f(truthy(self.eval(b)?)));
            }
            _ => {}
        }

        let x = self.eval(a)?;
        let y = self.eval(b)?;
        Ok(apply_binop(op, x, y))
    }

    fn eval_assign(
        &mut self,
        target: &Expr,
        op: Option<BinOp>,
        value: &Expr,
    ) -> Result<f64, EvalError> {
        let rhs = self.eval(value)?;
        match target {
            Expr::Var(name) => {
                let new = match op {
                    None => rhs,
                    Some(o) => {
                        let cur = self.vars.get(name).copied().unwrap_or(0.0);
                        apply_binop(o, cur, rhs)
                    }
                };
                self.vars.insert(name.clone(), new);
                Ok(new)
            }
            Expr::Mem { base, offset } => {
                let addr = self.mem_addr(base, offset.as_deref())?;
                let new = match op {
                    None => rhs,
                    Some(o) => {
                        let cur = self.megabuf.get(&addr).copied().unwrap_or(0.0);
                        apply_binop(o, cur, rhs)
                    }
                };
                self.megabuf.insert(addr, new);
                Ok(new)
            }
            // Parser guarantees only l-values reach here.
            _ => unreachable!("non-lvalue assignment target"),
        }
    }

    fn mem_addr(&mut self, base: &Expr, offset: Option<&Expr>) -> Result<i64, EvalError> {
        let b = self.eval(base)?;
        let o = match offset {
            Some(e) => self.eval(e)?,
            None => 0.0,
        };
        Ok((b + o) as i64)
    }

    fn eval_call(&mut self, name: &str, args: &[Expr]) -> Result<f64, EvalError> {
        // ---- Special forms: control flow with lazy argument evaluation. ----
        match name {
            "if" => {
                expect(name, args, 3)?;
                return if truthy(self.eval(&args[0])?) {
                    self.eval(&args[1])
                } else {
                    self.eval(&args[2])
                };
            }
            "loop" => {
                expect(name, args, 2)?;
                let n = self.eval(&args[0])?;
                let count = (n.max(0.0) as u64).min(MAX_ITERS);
                let mut last = 0.0;
                for _ in 0..count {
                    last = self.eval(&args[1])?;
                }
                return Ok(last);
            }
            "while" => {
                expect(name, args, 1)?;
                let mut last = 0.0;
                let mut iters = 0u64;
                loop {
                    let v = self.eval(&args[0])?;
                    if !truthy(v) {
                        break;
                    }
                    last = v;
                    iters += 1;
                    if iters >= MAX_ITERS {
                        break;
                    }
                }
                return Ok(last);
            }
            "exec2" => {
                expect(name, args, 2)?;
                self.eval(&args[0])?;
                return self.eval(&args[1]);
            }
            "exec3" => {
                expect(name, args, 3)?;
                self.eval(&args[0])?;
                self.eval(&args[1])?;
                return self.eval(&args[2]);
            }
            _ => {}
        }

        // ---- Strict functions: evaluate all arguments first. ----
        let a: Vec<f64> = args.iter().map(|e| self.eval(e)).collect::<Result<_, _>>()?;
        let n = a.len();

        macro_rules! arity {
            ($k:expr) => {
                if n != $k {
                    return Err(EvalError::Arity {
                        name: name.into(),
                        expected: stringify!($k),
                        got: n,
                    });
                }
            };
        }

        let v = match name {
            // Trig
            "sin" => { arity!(1); a[0].sin() }
            "cos" => { arity!(1); a[0].cos() }
            "tan" => { arity!(1); a[0].tan() }
            "asin" => { arity!(1); a[0].asin() }
            "acos" => { arity!(1); a[0].acos() }
            "atan" => { arity!(1); a[0].atan() }
            "atan2" => { arity!(2); a[0].atan2(a[1]) }
            "sinh" => { arity!(1); a[0].sinh() }
            "cosh" => { arity!(1); a[0].cosh() }
            "tanh" => { arity!(1); a[0].tanh() }

            // Powers / roots / exp / log
            "sqrt" => { arity!(1); if a[0] < 0.0 { 0.0 } else { a[0].sqrt() } }
            "sqr" => { arity!(1); a[0] * a[0] }
            "invsqrt" => { arity!(1); if a[0] <= 0.0 { 0.0 } else { 1.0 / a[0].sqrt() } }
            "pow" => { arity!(2); a[0].powf(a[1]) }
            "exp" => { arity!(1); a[0].exp() }
            "log" => { arity!(1); a[0].ln() }
            "log10" => { arity!(1); a[0].log10() }

            // Rounding / sign
            "abs" => { arity!(1); a[0].abs() }
            "floor" => { arity!(1); a[0].floor() }
            "ceil" => { arity!(1); a[0].ceil() }
            "int" => { arity!(1); a[0].trunc() }
            "sign" => { arity!(1); if a[0] > 0.0 { 1.0 } else if a[0] < 0.0 { -1.0 } else { 0.0 } }

            // Min / max / clamp-ish
            "min" => { arity!(2); a[0].min(a[1]) }
            "max" => { arity!(2); a[0].max(a[1]) }

            // Modulo (matches `%`: zero divisor -> 0)
            "fmod" => { arity!(2); safe_mod(a[0], a[1]) }

            // Boolean helpers (eager — distinct from && / ||)
            "band" => { arity!(2); bool_f(truthy(a[0]) && truthy(a[1])) }
            "bor" => { arity!(2); bool_f(truthy(a[0]) || truthy(a[1])) }
            "bnot" => { arity!(1); bool_f(!truthy(a[0])) }
            "equal" => { arity!(2); bool_f((a[0] - a[1]).abs() < EPS) }
            "above" => { arity!(2); bool_f(a[0] > a[1]) }
            "below" => { arity!(2); bool_f(a[0] < a[1]) }

            // Misc
            "sigmoid" => { arity!(2); 1.0 / (1.0 + (-a[0] * a[1]).exp()) }
            "rand" => {
                arity!(1);
                let m = a[0];
                if m <= 0.0 { 0.0 } else { (self.next_rand() * m).floor() }
            }

            // Explicit memory accessors.
            "megabuf" => { arity!(1); self.megabuf.get(&(a[0] as i64)).copied().unwrap_or(0.0) }
            "gmegabuf" => { arity!(1); self.gmegabuf.get(&(a[0] as i64)).copied().unwrap_or(0.0) }

            other => return Err(EvalError::UnknownFunction(other.into())),
        };
        Ok(v)
    }
}

fn expect(name: &str, args: &[Expr], k: usize) -> Result<(), EvalError> {
    if args.len() == k {
        Ok(())
    } else {
        Err(EvalError::Arity {
            name: name.into(),
            expected: match k {
                1 => "1",
                2 => "2",
                3 => "3",
                _ => "N",
            },
            got: args.len(),
        })
    }
}

fn bool_f(b: bool) -> f64 {
    if b {
        1.0
    } else {
        0.0
    }
}

fn safe_div(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        0.0
    } else {
        a / b
    }
}

fn safe_mod(a: f64, b: f64) -> f64 {
    if b == 0.0 {
        0.0
    } else {
        a % b
    }
}

fn apply_binop(op: BinOp, x: f64, y: f64) -> f64 {
    use BinOp::*;
    match op {
        Add => x + y,
        Sub => x - y,
        Mul => x * y,
        Div => safe_div(x, y),
        Mod => safe_mod(x, y),
        Pow => x.powf(y),
        Eq => bool_f((x - y).abs() < EPS),
        Ne => bool_f((x - y).abs() >= EPS),
        Lt => bool_f(x < y),
        Gt => bool_f(x > y),
        Le => bool_f(x <= y),
        Ge => bool_f(x >= y),
        And => bool_f(truthy(x) && truthy(y)),
        Or => bool_f(truthy(x) || truthy(y)),
        BitAnd => ((x as i64) & (y as i64)) as f64,
        BitOr => ((x as i64) | (y as i64)) as f64,
    }
}
