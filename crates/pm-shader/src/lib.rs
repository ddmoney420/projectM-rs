//! `pm-shader` — translates Milkdrop preset shaders (a DirectX9-era HLSL
//! dialect) into WGSL for wgpu.
//!
//! projectM transpiles preset warp/composite shaders from HLSL to GLSL using a
//! vendored `hlslparser`. This crate does the equivalent HLSL → **WGSL**, so the
//! same presets run on the wgpu backend (native Metal/Vulkan/DX).
//!
//! Pipeline: [`preprocess`] (`#define` expansion) → HLSL lex/parse → WGSL codegen.

mod ast;
mod codegen;
mod lexer;
mod parser;
mod preprocess;

pub use ast::*;
pub use codegen::generate;
pub use lexer::{lex, LexError, Tok};
pub use parser::{parse, ParseError};
pub use preprocess::preprocess;

/// Translate Milkdrop HLSL preset shader source into WGSL.
///
/// Runs the full pipeline: `#define` preprocessing → HLSL parse → WGSL codegen.
/// Global uniforms are emitted as `var<private>` placeholders so the result is a
/// self-contained module; the preset engine replaces these with real bindings.
pub fn translate(src: &str) -> Result<String, ParseError> {
    let preprocessed = preprocess(src);
    let items = parse(&preprocessed)?;
    Ok(generate(&items))
}
