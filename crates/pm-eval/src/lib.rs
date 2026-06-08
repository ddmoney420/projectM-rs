//! `pm-eval` — a Rust port of [projectm-eval], the Milkdrop / ns-eel expression
//! language used by every `.milk` preset for its `per_frame`, `per_pixel`,
//! `per_point` and custom-shape/waveform equations.
//!
//! [projectm-eval]: https://github.com/projectM-visualizer/projectm-eval
//!
//! A program is **compiled against a [`Context`]**: variable names are resolved
//! to slot indices and functions to an opcode enum once, so executing the
//! program (potentially thousands of times per frame) needs no hashing or string
//! matching. Run a program only against the context it was compiled with.
//!
//! # Example
//!
//! ```
//! use pm_eval::{Program, Context};
//!
//! let mut ctx = Context::new();
//! // Compile once (against the context), run every frame.
//! let prog = Program::compile(&mut ctx, "y = sin(x) * amp; y * 2").unwrap();
//!
//! ctx.set("x", std::f64::consts::FRAC_PI_2); // sin = 1
//! ctx.set("amp", 3.0);
//!
//! let out = prog.run(&mut ctx).unwrap();
//! assert!((out - 6.0).abs() < 1e-9);
//! assert!((ctx.get("y") - 3.0).abs() < 1e-9); // side effect persisted
//! ```

mod ast;
mod compile;
mod interp;
mod lexer;
mod parser;

pub use ast::{BinOp, Expr, UnOp};
pub use interp::{Context, EvalError, VarSlot};
pub use lexer::{lex, LexError, Tok};
pub use parser::{parse, ParseError};

use compile::{resolve, Node};

/// A compiled program: a resolved [`Node`] tree ready to run against the
/// [`Context`] it was compiled with.
#[derive(Debug, Clone)]
pub struct Program {
    root: Node,
    source: String,
}

impl Program {
    /// Parse and compile source text against `ctx`, resolving variable names to
    /// the context's slots. Returns a [`ParseError`] on malformed input.
    pub fn compile(ctx: &mut Context, src: &str) -> Result<Program, ParseError> {
        let ast = parse(src)?;
        Ok(Program { root: resolve(ctx, &ast), source: src.to_owned() })
    }

    /// The original source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Execute against `ctx`, returning the value of the last statement.
    pub fn run(&self, ctx: &mut Context) -> Result<f64, EvalError> {
        ctx.run(&self.root)
    }
}

impl Context {
    /// Convenience: compile and run a one-off expression against this context.
    pub fn eval_str(&mut self, src: &str) -> Result<f64, Box<dyn std::error::Error>> {
        let prog = Program::compile(self, src)?;
        Ok(prog.run(self)?)
    }
}
