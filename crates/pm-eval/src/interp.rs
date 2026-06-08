//! Execution context and the executor for compiled [`Node`] programs.
//!
//! Variables live in a `Vec<f64>` indexed by slot (resolved at compile time),
//! so reads/writes are a plain index — no per-access hashing. The host can hold
//! a [`VarSlot`] handle to set/get hot variables (e.g. per-pixel `x`/`y`)
//! without a name lookup either.
//!
//! Semantics mirror ns-eel / projectm-eval quirks preset authors rely on:
//!   * division / modulo by zero yields `0.0`,
//!   * `==` / `!=` compare with an epsilon of `1e-5`,
//!   * truthiness is `|v| > 1e-5`.

use crate::ast::{BinOp, UnOp};
use crate::compile::{CallOp, Node, Target};
use std::collections::HashMap;
use std::fmt;

const EPS: f64 = 1e-5;
const MAX_ITERS: u64 = 1 << 21;

fn truthy(v: f64) -> bool {
    v.abs() > EPS
}

#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    UnknownFunction(String),
    Arity { name: &'static str, expected: &'static str, got: usize },
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

/// A handle to a variable's storage slot, for fast repeated set/get from the
/// host in hot loops. Obtain via [`Context::variable_slot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VarSlot(usize);

/// Execution state: variables (slot-indexed), per-instance and global memory,
/// and the RNG.
#[derive(Debug, Clone)]
pub struct Context {
    slots: Vec<f64>,
    names: HashMap<String, usize>,
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
            slots: Vec::new(),
            names: HashMap::new(),
            megabuf: HashMap::new(),
            gmegabuf: HashMap::new(),
            rng: 0x9E37_79B9_7F4A_7C15,
        };
        // Named constants (over-writable, like ns-eel).
        ctx.set("pi", std::f64::consts::PI);
        ctx.set("e", std::f64::consts::E);
        ctx.set("phi", 1.618_033_988_749_895);
        ctx
    }

    /// Get or create the slot index for a (case-insensitive) variable name.
    pub(crate) fn intern(&mut self, name: &str) -> usize {
        let lower = name.to_ascii_lowercase();
        if let Some(&i) = self.names.get(&lower) {
            return i;
        }
        let i = self.slots.len();
        self.slots.push(0.0);
        self.names.insert(lower, i);
        i
    }

    /// A reusable handle to a variable's slot, for fast set/get in hot loops.
    pub fn variable_slot(&mut self, name: &str) -> VarSlot {
        VarSlot(self.intern(name))
    }

    pub fn seed(&mut self, seed: u64) {
        self.rng = seed | 1;
    }

    pub fn set(&mut self, name: &str, value: f64) {
        let i = self.intern(name);
        self.slots[i] = value;
    }

    pub fn get(&self, name: &str) -> f64 {
        self.names.get(&name.to_ascii_lowercase()).map(|&i| self.slots[i]).unwrap_or(0.0)
    }

    /// Fast set via a pre-resolved slot handle.
    #[inline]
    pub fn set_slot(&mut self, slot: VarSlot, value: f64) {
        self.slots[slot.0] = value;
    }

    /// Fast get via a pre-resolved slot handle.
    #[inline]
    pub fn get_slot(&self, slot: VarSlot) -> f64 {
        self.slots[slot.0]
    }

    fn next_rand(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Execute a compiled program, returning the value of the last statement.
    pub(crate) fn run(&mut self, program: &Node) -> Result<f64, EvalError> {
        self.execute(program)
    }

    fn execute(&mut self, n: &Node) -> Result<f64, EvalError> {
        match n {
            Node::Num(v) => Ok(*v),
            Node::Var(i) => Ok(self.slots[*i]),
            Node::Mem(base, offset) => {
                let addr = self.mem_addr(base, offset.as_deref())?;
                Ok(self.megabuf.get(&addr).copied().unwrap_or(0.0))
            }
            Node::Unary(op, x) => {
                let v = self.execute(x)?;
                Ok(match op {
                    UnOp::Neg => -v,
                    UnOp::Not => bool_f(!truthy(v)),
                })
            }
            Node::Binary(op, a, b) => self.eval_binary(*op, a, b),
            Node::Assign(target, op, value) => self.eval_assign(target, *op, value),
            Node::Block(stmts) => {
                let mut last = 0.0;
                for s in stmts {
                    last = self.execute(s)?;
                }
                Ok(last)
            }
            Node::Call(op, args) => self.eval_call(op, args),
        }
    }

    fn eval_binary(&mut self, op: BinOp, a: &Node, b: &Node) -> Result<f64, EvalError> {
        match op {
            BinOp::And => Ok(bool_f(truthy(self.execute(a)?) && truthy(self.execute(b)?))),
            BinOp::Or => {
                if truthy(self.execute(a)?) {
                    Ok(1.0)
                } else {
                    Ok(bool_f(truthy(self.execute(b)?)))
                }
            }
            _ => {
                let x = self.execute(a)?;
                let y = self.execute(b)?;
                Ok(apply_binop(op, x, y))
            }
        }
    }

    fn eval_assign(&mut self, target: &Target, op: Option<BinOp>, value: &Node) -> Result<f64, EvalError> {
        let rhs = self.execute(value)?;
        match target {
            Target::Var(i) => {
                let new = match op {
                    None => rhs,
                    Some(o) => apply_binop(o, self.slots[*i], rhs),
                };
                self.slots[*i] = new;
                Ok(new)
            }
            Target::Mem(base, offset) => {
                let addr = self.mem_addr(base, offset.as_deref())?;
                let new = match op {
                    None => rhs,
                    Some(o) => apply_binop(o, self.megabuf.get(&addr).copied().unwrap_or(0.0), rhs),
                };
                self.megabuf.insert(addr, new);
                Ok(new)
            }
            Target::MegaCall(global, idx) => {
                let i = self.execute(idx)? as i64;
                let cur = if *global {
                    self.gmegabuf.get(&i).copied().unwrap_or(0.0)
                } else {
                    self.megabuf.get(&i).copied().unwrap_or(0.0)
                };
                let new = match op {
                    None => rhs,
                    Some(o) => apply_binop(o, cur, rhs),
                };
                if *global {
                    self.gmegabuf.insert(i, new);
                } else {
                    self.megabuf.insert(i, new);
                }
                Ok(new)
            }
        }
    }

    fn mem_addr(&mut self, base: &Node, offset: Option<&Node>) -> Result<i64, EvalError> {
        let b = self.execute(base)?;
        let o = match offset {
            Some(e) => self.execute(e)?,
            None => 0.0,
        };
        Ok((b + o) as i64)
    }

    fn eval_call(&mut self, op: &CallOp, args: &[Node]) -> Result<f64, EvalError> {
        use CallOp::*;

        // ---- Special forms: lazy argument evaluation. ----
        match op {
            If => {
                arity(op, args, 3)?;
                return if truthy(self.execute(&args[0])?) {
                    self.execute(&args[1])
                } else {
                    self.execute(&args[2])
                };
            }
            Loop => {
                arity(op, args, 2)?;
                let count = (self.execute(&args[0])?.max(0.0) as u64).min(MAX_ITERS);
                let mut last = 0.0;
                for _ in 0..count {
                    last = self.execute(&args[1])?;
                }
                return Ok(last);
            }
            While => {
                arity(op, args, 1)?;
                let mut last = 0.0;
                let mut iters = 0u64;
                loop {
                    let v = self.execute(&args[0])?;
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
            Exec2 => {
                arity(op, args, 2)?;
                self.execute(&args[0])?;
                return self.execute(&args[1]);
            }
            Exec3 => {
                arity(op, args, 3)?;
                self.execute(&args[0])?;
                self.execute(&args[1])?;
                return self.execute(&args[2]);
            }
            Assign => {
                arity(op, args, 2)?;
                return match &args[0] {
                    Node::Var(i) => {
                        let v = self.execute(&args[1])?;
                        self.slots[*i] = v;
                        Ok(v)
                    }
                    Node::Mem(base, offset) => {
                        let addr = self.mem_addr(base, offset.as_deref())?;
                        let v = self.execute(&args[1])?;
                        self.megabuf.insert(addr, v);
                        Ok(v)
                    }
                    Node::Call(CallOp::Megabuf, a) if a.len() == 1 => {
                        let idx = self.execute(&a[0])? as i64;
                        let v = self.execute(&args[1])?;
                        self.megabuf.insert(idx, v);
                        Ok(v)
                    }
                    Node::Call(CallOp::Gmegabuf, a) if a.len() == 1 => {
                        let idx = self.execute(&a[0])? as i64;
                        let v = self.execute(&args[1])?;
                        self.gmegabuf.insert(idx, v);
                        Ok(v)
                    }
                    _ => self.execute(&args[1]),
                };
            }
            _ => {}
        }

        // ---- Strict functions: evaluate all arguments first. ----
        // All built-ins take <= 2 args, so evaluate into a stack buffer instead
        // of allocating a Vec per call (the hot path runs thousands of times
        // per frame).
        let len = args.len();
        let mut buf = [0.0f64; 4];
        for (i, arg) in args.iter().enumerate().take(4) {
            buf[i] = self.execute(arg)?;
        }
        let a = &buf;

        macro_rules! arity1 {
            ($name:expr, $k:expr) => {
                if len != $k {
                    return Err(EvalError::Arity { name: $name, expected: stringify!($k), got: len });
                }
            };
        }

        let v = match op {
            Sin => { arity1!("sin", 1); a[0].sin() }
            Cos => { arity1!("cos", 1); a[0].cos() }
            Tan => { arity1!("tan", 1); a[0].tan() }
            Asin => { arity1!("asin", 1); a[0].asin() }
            Acos => { arity1!("acos", 1); a[0].acos() }
            Atan => { arity1!("atan", 1); a[0].atan() }
            Atan2 => { arity1!("atan2", 2); a[0].atan2(a[1]) }
            Sinh => { arity1!("sinh", 1); a[0].sinh() }
            Cosh => { arity1!("cosh", 1); a[0].cosh() }
            Tanh => { arity1!("tanh", 1); a[0].tanh() }
            Sqrt => { arity1!("sqrt", 1); if a[0] < 0.0 { 0.0 } else { a[0].sqrt() } }
            Sqr => { arity1!("sqr", 1); a[0] * a[0] }
            Invsqrt => { arity1!("invsqrt", 1); if a[0] <= 0.0 { 0.0 } else { 1.0 / a[0].sqrt() } }
            Pow => { arity1!("pow", 2); a[0].powf(a[1]) }
            Exp => { arity1!("exp", 1); a[0].exp() }
            Log => { arity1!("log", 1); a[0].ln() }
            Log10 => { arity1!("log10", 1); a[0].log10() }
            Abs => { arity1!("abs", 1); a[0].abs() }
            Floor => { arity1!("floor", 1); a[0].floor() }
            Ceil => { arity1!("ceil", 1); a[0].ceil() }
            Int => { arity1!("int", 1); a[0].trunc() }
            Sign => { arity1!("sign", 1); if a[0] > 0.0 { 1.0 } else if a[0] < 0.0 { -1.0 } else { 0.0 } }
            Min => { arity1!("min", 2); a[0].min(a[1]) }
            Max => { arity1!("max", 2); a[0].max(a[1]) }
            Fmod => { arity1!("fmod", 2); safe_mod(a[0], a[1]) }
            Band => { arity1!("band", 2); bool_f(truthy(a[0]) && truthy(a[1])) }
            Bor => { arity1!("bor", 2); bool_f(truthy(a[0]) || truthy(a[1])) }
            Bnot => { arity1!("bnot", 1); bool_f(!truthy(a[0])) }
            Equal => { arity1!("equal", 2); bool_f((a[0] - a[1]).abs() < EPS) }
            Above => { arity1!("above", 2); bool_f(a[0] > a[1]) }
            Below => { arity1!("below", 2); bool_f(a[0] < a[1]) }
            Sigmoid => { arity1!("sigmoid", 2); 1.0 / (1.0 + (-a[0] * a[1]).exp()) }
            Rand => {
                arity1!("rand", 1);
                if a[0] <= 0.0 { 0.0 } else { (self.next_rand() * a[0]).floor() }
            }
            Megabuf => { arity1!("megabuf", 1); self.megabuf.get(&(a[0] as i64)).copied().unwrap_or(0.0) }
            Gmegabuf => { arity1!("gmegabuf", 1); self.gmegabuf.get(&(a[0] as i64)).copied().unwrap_or(0.0) }
            Unknown(name) => return Err(EvalError::UnknownFunction(name.clone())),
            If | Loop | While | Exec2 | Exec3 | Assign => unreachable!("special forms handled above"),
        };
        Ok(v)
    }
}

fn arity(op: &CallOp, args: &[Node], k: usize) -> Result<(), EvalError> {
    if args.len() == k {
        return Ok(());
    }
    let name = match op {
        CallOp::If => "if",
        CallOp::Loop => "loop",
        CallOp::While => "while",
        CallOp::Exec2 => "exec2",
        CallOp::Exec3 => "exec3",
        CallOp::Assign => "assign",
        _ => "function",
    };
    let expected = match k {
        1 => "1",
        2 => "2",
        3 => "3",
        _ => "N",
    };
    Err(EvalError::Arity { name, expected, got: args.len() })
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
