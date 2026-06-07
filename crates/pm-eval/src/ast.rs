//! Abstract syntax tree for compiled expressions.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    /// `&&` — logical, value is 1.0 / 0.0
    And,
    /// `||`
    Or,
    /// `&` — bitwise on truncated integers
    BitAnd,
    /// `|`
    BitOr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    /// `!x` — logical not
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    /// Variable reference, name already lowercased.
    Var(String),
    /// Megabuf access `base[offset]` (offset defaults to 0 for `base[]`).
    /// Effective address is `base + offset` truncated to an integer.
    Mem {
        base: Box<Expr>,
        offset: Option<Box<Expr>>,
    },
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// Assignment to a variable or megabuf cell. `op` is `None` for `=`,
    /// or `Some(_)` for compound assignment (`+=`, `*=`, ...).
    Assign {
        target: Box<Expr>,
        op: Option<BinOp>,
        value: Box<Expr>,
    },
    /// Function call or special form (`if`, `loop`, `while`, `exec2`, ...).
    Call(String, Vec<Expr>),
    /// `;`-separated statement sequence; value is the last statement.
    Block(Vec<Expr>),
}
