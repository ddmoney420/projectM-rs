//! Error types for preset loading and evaluation.

use std::fmt;

#[derive(Debug)]
pub enum PresetError {
    /// A `.milk` file failed to parse into key/value pairs.
    InvalidFile,
    /// An equation block failed to compile.
    Compile { block: &'static str, source: pm_eval::ParseError },
    /// An equation block failed at runtime.
    Eval { block: &'static str, source: pm_eval::EvalError },
}

impl PresetError {
    pub fn compile(block: &'static str, source: pm_eval::ParseError) -> Self {
        PresetError::Compile { block, source }
    }
    pub fn eval(block: &'static str, source: pm_eval::EvalError) -> Self {
        PresetError::Eval { block, source }
    }
}

impl fmt::Display for PresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PresetError::InvalidFile => write!(f, "not a valid .milk preset file"),
            PresetError::Compile { block, source } => {
                write!(f, "failed to compile {block} code: {source}")
            }
            PresetError::Eval { block, source } => {
                write!(f, "error executing {block} code: {source}")
            }
        }
    }
}

impl std::error::Error for PresetError {}
