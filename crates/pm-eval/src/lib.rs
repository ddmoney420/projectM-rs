//! `pm-eval` — a Rust port of [projectm-eval], the Milkdrop / ns-eel expression
//! language used by every `.milk` preset for its `per_frame`, `per_pixel`,
//! `per_point` and custom-shape/waveform equations.
//!
//! [projectm-eval]: https://github.com/projectM-visualizer/projectm-eval
//!
//! # Example
//!
//! ```
//! use pm_eval::{Program, Context};
//!
//! // Compile once, run every frame.
//! let prog = Program::compile("y = sin(x) * amp; y * 2").unwrap();
//!
//! let mut ctx = Context::new();
//! ctx.set("x", std::f64::consts::FRAC_PI_2); // sin = 1
//! ctx.set("amp", 3.0);
//!
//! let out = prog.run(&mut ctx).unwrap();
//! assert!((out - 6.0).abs() < 1e-9);
//! assert!((ctx.get("y") - 3.0).abs() < 1e-9); // side effect persisted
//! ```

mod ast;
mod interp;
mod lexer;
mod parser;

pub use ast::{BinOp, Expr, UnOp};
pub use interp::{Context, EvalError};
pub use lexer::{lex, LexError, Tok};
pub use parser::{parse, ParseError};

/// A compiled program: a parsed [`Expr`] tree ready to run against a [`Context`].
///
/// Compile a preset's equation block once, then execute it cheaply per frame.
#[derive(Debug, Clone)]
pub struct Program {
    root: Expr,
    source: String,
}

impl Program {
    /// Parse and compile source text. Returns a [`ParseError`] on malformed input.
    pub fn compile(src: &str) -> Result<Program, ParseError> {
        Ok(Program {
            root: parse(src)?,
            source: src.to_owned(),
        })
    }

    /// The original source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The root AST node (handy for inspection / further compilation passes).
    pub fn ast(&self) -> &Expr {
        &self.root
    }

    /// Execute against `ctx`, returning the value of the last statement.
    pub fn run(&self, ctx: &mut Context) -> Result<f64, EvalError> {
        ctx.run(&self.root)
    }
}

impl Context {
    /// Convenience: compile and run a one-off expression against this context.
    pub fn eval_str(&mut self, src: &str) -> Result<f64, Box<dyn std::error::Error>> {
        let prog = Program::compile(src)?;
        Ok(prog.run(self)?)
    }
}
