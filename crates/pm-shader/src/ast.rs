//! AST for the Milkdrop HLSL dialect.

/// HLSL types used by preset shaders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Void,
    Float,
    Int,
    Bool,
    Float2,
    Float3,
    Float4,
    Int2,
    Int3,
    Int4,
    Bool2,
    Bool3,
    Bool4,
    /// `floatNxN` square matrices.
    Mat2,
    Mat3,
    Mat4,
    /// `float4x3` / `float3x4` (rows x cols, HLSL convention).
    Mat4x3,
    Mat3x4,
    Sampler2D,
    Sampler3D,
}

impl Type {
    /// Component count for a vector type, else `None`.
    pub fn vector_len(self) -> Option<u8> {
        Some(match self {
            Type::Float2 | Type::Int2 | Type::Bool2 => 2,
            Type::Float3 | Type::Int3 | Type::Bool3 => 3,
            Type::Float4 | Type::Int4 | Type::Bool4 => 4,
            _ => return None,
        })
    }

    pub fn is_scalar(self) -> bool {
        matches!(self, Type::Float | Type::Int | Type::Bool)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    IntLit(i64),
    FloatLit(f64),
    BoolLit(bool),
    Ident(String),
    Unary(UnOp, Box<Expr>),
    PostInc(Box<Expr>),
    PostDec(Box<Expr>),
    PreInc(Box<Expr>),
    PreDec(Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Assign(AssignOp, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Member / swizzle access: `expr.field`.
    Member(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// Function or intrinsic call.
    Call(String, Vec<Expr>),
    /// Type constructor, e.g. `float3(1, 2, 3)`.
    Construct(Type, Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// A single declarator: `ty name (= init)?`.
    Decl {
        ty: Type,
        name: String,
        init: Option<Expr>,
    },
    Expr(Expr),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    For(Option<Box<Stmt>>, Option<Expr>, Option<Expr>, Box<Stmt>),
    While(Expr, Box<Stmt>),
    Return(Option<Expr>),
    Block(Vec<Stmt>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamQual {
    In,
    Out,
    InOut,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub qualifier: ParamQual,
    pub ty: Type,
    pub name: String,
    pub semantic: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub ret: Type,
    pub name: String,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Function(Function),
    /// A global variable (often `uniform`).
    Global {
        uniform: bool,
        ty: Type,
        name: String,
        semantic: Option<String>,
        init: Option<Expr>,
    },
    /// A combined-sampler declaration, e.g. `sampler2D sampler_main;`.
    Sampler {
        ty: Type,
        name: String,
    },
}
