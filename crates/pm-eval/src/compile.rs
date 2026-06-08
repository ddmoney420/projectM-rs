//! Compiled intermediate representation.
//!
//! The parser produces an [`Expr`] tree with variables as strings and functions
//! as names. Compilation resolves those once: variables become **slot indices**
//! (a `Vec` lookup, no hashing) and functions become a [`CallOp`] enum (no
//! string match). The executor then walks this [`Node`] tree — the same shape
//! as the AST, but with the per-access overhead removed. This is what makes
//! per-pixel / per-point / per-instance code (run thousands of times per frame)
//! fast.

use crate::ast::{BinOp, Expr, UnOp};
use crate::interp::Context;

/// A compiled expression node.
#[derive(Debug, Clone)]
pub enum Node {
    Num(f64),
    /// Variable slot index.
    Var(usize),
    /// `megabuf[base (+ offset)]`.
    Mem(Box<Node>, Option<Box<Node>>),
    Unary(UnOp, Box<Node>),
    Binary(BinOp, Box<Node>, Box<Node>),
    Assign(Target, Option<BinOp>, Box<Node>),
    Block(Vec<Node>),
    Call(CallOp, Vec<Node>),
}

/// A compiled assignment target.
#[derive(Debug, Clone)]
pub enum Target {
    Var(usize),
    Mem(Box<Node>, Option<Box<Node>>),
    /// `megabuf(i)` (false) / `gmegabuf(i)` (true).
    MegaCall(bool, Box<Node>),
}

/// A resolved function / special form.
#[derive(Debug, Clone, PartialEq)]
pub enum CallOp {
    // Special forms (lazy argument evaluation).
    If,
    Loop,
    While,
    Exec2,
    Exec3,
    Assign,
    // Strict math functions.
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Sinh,
    Cosh,
    Tanh,
    Sqrt,
    Sqr,
    Invsqrt,
    Pow,
    Exp,
    Log,
    Log10,
    Abs,
    Floor,
    Ceil,
    Int,
    Sign,
    Min,
    Max,
    Fmod,
    Band,
    Bor,
    Bnot,
    Equal,
    Above,
    Below,
    Sigmoid,
    Rand,
    Megabuf,
    Gmegabuf,
    /// Unresolved name — reported as an error at runtime.
    Unknown(String),
}

fn name_to_callop(name: &str) -> CallOp {
    use CallOp::*;
    match name {
        "if" => If,
        "loop" => Loop,
        "while" => While,
        "exec2" => Exec2,
        "exec3" => Exec3,
        "assign" => Assign,
        "sin" => Sin,
        "cos" => Cos,
        "tan" => Tan,
        "asin" => Asin,
        "acos" => Acos,
        "atan" => Atan,
        "atan2" => Atan2,
        "sinh" => Sinh,
        "cosh" => Cosh,
        "tanh" => Tanh,
        "sqrt" => Sqrt,
        "sqr" => Sqr,
        "invsqrt" => Invsqrt,
        "pow" => Pow,
        "exp" => Exp,
        "log" => Log,
        "log10" => Log10,
        "abs" => Abs,
        "floor" => Floor,
        "ceil" => Ceil,
        "int" => Int,
        "sign" => Sign,
        "min" => Min,
        "max" => Max,
        "fmod" => Fmod,
        "band" => Band,
        "bor" => Bor,
        "bnot" => Bnot,
        "equal" => Equal,
        "above" => Above,
        "below" => Below,
        "sigmoid" => Sigmoid,
        "rand" => Rand,
        "megabuf" => Megabuf,
        "gmegabuf" => Gmegabuf,
        other => Unknown(other.to_string()),
    }
}

/// Resolve an AST expression against `ctx`'s variable-slot table.
pub fn resolve(ctx: &mut Context, e: &Expr) -> Node {
    match e {
        Expr::Num(n) => Node::Num(*n),
        Expr::Var(name) => Node::Var(ctx.intern(name)),
        Expr::Mem { base, offset } => Node::Mem(
            Box::new(resolve(ctx, base)),
            offset.as_ref().map(|o| Box::new(resolve(ctx, o))),
        ),
        Expr::Unary(op, x) => Node::Unary(*op, Box::new(resolve(ctx, x))),
        Expr::Binary(op, a, b) => {
            Node::Binary(*op, Box::new(resolve(ctx, a)), Box::new(resolve(ctx, b)))
        }
        Expr::Assign { target, op, value } => {
            Node::Assign(resolve_target(ctx, target), *op, Box::new(resolve(ctx, value)))
        }
        Expr::Block(stmts) => Node::Block(stmts.iter().map(|s| resolve(ctx, s)).collect()),
        Expr::Call(name, args) => {
            Node::Call(name_to_callop(name), args.iter().map(|a| resolve(ctx, a)).collect())
        }
    }
}

/// Resolve an l-value target. The parser guarantees only valid targets reach
/// assignment, so the fallback is a harmless variable slot.
fn resolve_target(ctx: &mut Context, e: &Expr) -> Target {
    match e {
        Expr::Var(name) => Target::Var(ctx.intern(name)),
        Expr::Mem { base, offset } => Target::Mem(
            Box::new(resolve(ctx, base)),
            offset.as_ref().map(|o| Box::new(resolve(ctx, o))),
        ),
        Expr::Call(name, args) if args.len() == 1 && (name == "megabuf" || name == "gmegabuf") => {
            Target::MegaCall(name == "gmegabuf", Box::new(resolve(ctx, &args[0])))
        }
        // Unreachable for parser output; intern a scratch name to stay total.
        _ => Target::Var(ctx.intern("__invalid_target")),
    }
}
