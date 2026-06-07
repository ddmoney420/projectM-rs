//! Pratt (precedence-climbing) parser producing an [`Expr`] tree.

use crate::ast::{BinOp, Expr, UnOp};
use crate::lexer::{lex, LexError, Tok};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    Lex(LexError),
    Unexpected { got: Tok, expected: &'static str },
    TrailingTokens(Tok),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Lex(e) => write!(f, "{e}"),
            ParseError::Unexpected { got, expected } => {
                write!(f, "unexpected {got}, expected {expected}")
            }
            ParseError::TrailingTokens(t) => write!(f, "trailing token {t}"),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError::Lex(e)
    }
}

/// Parse a full program (a `;`-separated statement block) into one [`Expr`].
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let toks = lex(src)?;
    let mut p = Parser { toks, pos: 0 };
    let block = p.parse_block(&[Tok::Eof])?;
    match p.peek() {
        Tok::Eof => Ok(block),
        other => Err(ParseError::TrailingTokens(other.clone())),
    }
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos]
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].clone();
        self.pos += 1;
        t
    }

    fn eat(&mut self, want: &Tok, expected: &'static str) -> Result<(), ParseError> {
        if self.peek() == want {
            self.pos += 1;
            Ok(())
        } else {
            Err(ParseError::Unexpected { got: self.peek().clone(), expected })
        }
    }

    /// Parse statements separated by `;` until one of `terminators` is seen.
    /// A trailing `;` (empty final statement) is allowed and ignored.
    fn parse_block(&mut self, terminators: &[Tok]) -> Result<Expr, ParseError> {
        let mut stmts = Vec::new();
        loop {
            if terminators.contains(self.peek()) {
                break;
            }
            stmts.push(self.parse_expr(0)?);
            match self.peek() {
                Tok::Semicolon => {
                    self.pos += 1;
                    continue;
                }
                t if terminators.contains(t) => break,
                other => {
                    return Err(ParseError::Unexpected {
                        got: other.clone(),
                        expected: "';' or end of block",
                    })
                }
            }
        }
        if stmts.len() == 1 {
            Ok(stmts.pop().unwrap())
        } else {
            Ok(Expr::Block(stmts))
        }
    }

    /// Precedence-climbing expression parser.
    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_prefix()?;

        loop {
            // Postfix megabuf indexing binds tightest.
            if matches!(self.peek(), Tok::LBracket) {
                self.pos += 1;
                let offset = if matches!(self.peek(), Tok::RBracket) {
                    None
                } else {
                    Some(Box::new(self.parse_expr(0)?))
                };
                self.eat(&Tok::RBracket, "']'")?;
                lhs = Expr::Mem { base: Box::new(lhs), offset };
                continue;
            }

            let Some((op, l_bp, r_bp, assign)) = infix_binding(self.peek()) else {
                break;
            };
            if l_bp < min_bp {
                break;
            }
            self.pos += 1; // consume operator

            if assign {
                // Right-associative assignment; lhs must be an l-value.
                if !is_lvalue(&lhs) {
                    return Err(ParseError::Unexpected {
                        got: Tok::Assign,
                        expected: "assignable target before '='",
                    });
                }
                let value = self.parse_expr(r_bp)?;
                lhs = Expr::Assign { target: Box::new(lhs), op, value: Box::new(value) };
            } else {
                let rhs = self.parse_expr(r_bp)?;
                lhs = Expr::Binary(op.unwrap(), Box::new(lhs), Box::new(rhs));
            }
        }

        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Expr, ParseError> {
        match self.peek().clone() {
            Tok::Minus => {
                self.pos += 1;
                let e = self.parse_expr(PREFIX_BP)?;
                Ok(Expr::Unary(UnOp::Neg, Box::new(e)))
            }
            Tok::Plus => {
                // Unary plus is a no-op.
                self.pos += 1;
                self.parse_expr(PREFIX_BP)
            }
            Tok::Bang => {
                self.pos += 1;
                let e = self.parse_expr(PREFIX_BP)?;
                Ok(Expr::Unary(UnOp::Not, Box::new(e)))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.bump() {
            Tok::Num(n) => Ok(Expr::Num(n)),
            Tok::LParen => {
                // Parenthesized group may itself contain `;`-separated stmts.
                let inner = self.parse_block(&[Tok::RParen])?;
                self.eat(&Tok::RParen, "')'")?;
                Ok(inner)
            }
            Tok::Ident(name) => {
                if matches!(self.peek(), Tok::LParen) {
                    self.pos += 1;
                    let args = self.parse_args()?;
                    self.eat(&Tok::RParen, "')'")?;
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            other => Err(ParseError::Unexpected { got: other, expected: "expression" }),
        }
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if matches!(self.peek(), Tok::RParen) {
            return Ok(args);
        }
        loop {
            // Each argument is its own `;`-separated block, so control-flow
            // forms like `loop(n, a; b; c)` parse correctly.
            args.push(self.parse_block(&[Tok::Comma, Tok::RParen])?);
            match self.peek() {
                Tok::Comma => {
                    self.pos += 1;
                    continue;
                }
                _ => break,
            }
        }
        Ok(args)
    }
}

fn is_lvalue(e: &Expr) -> bool {
    matches!(e, Expr::Var(_) | Expr::Mem { .. })
}

// Prefix operators bind tighter than the binary arithmetic operators but
// looser than `^`, so `-2^2` parses as `-(2^2) = -4` (matching ns-eel).
const PREFIX_BP: u8 = 90;

/// Returns `(binop, left_bp, right_bp, is_assignment)` for an infix token.
/// `binop` is `None` only for plain `=`. Higher bp binds tighter.
fn infix_binding(t: &Tok) -> Option<(Option<BinOp>, u8, u8, bool)> {
    use BinOp::*;
    let r = match t {
        // Assignment — lowest, right-associative.
        Tok::Assign => (None, 5, 4, true),
        Tok::AddAssign => (Some(Add), 5, 4, true),
        Tok::SubAssign => (Some(Sub), 5, 4, true),
        Tok::MulAssign => (Some(Mul), 5, 4, true),
        Tok::DivAssign => (Some(Div), 5, 4, true),
        Tok::ModAssign => (Some(Mod), 5, 4, true),

        Tok::OrOr => (Some(Or), 10, 11, false),
        Tok::AndAnd => (Some(And), 20, 21, false),
        Tok::Pipe => (Some(BitOr), 30, 31, false),
        Tok::Amp => (Some(BitAnd), 40, 41, false),

        Tok::Eq => (Some(Eq), 50, 51, false),
        Tok::Ne => (Some(Ne), 50, 51, false),

        Tok::Lt => (Some(Lt), 60, 61, false),
        Tok::Gt => (Some(Gt), 60, 61, false),
        Tok::Le => (Some(Le), 60, 61, false),
        Tok::Ge => (Some(Ge), 60, 61, false),

        Tok::Plus => (Some(Add), 70, 71, false),
        Tok::Minus => (Some(Sub), 70, 71, false),

        Tok::Star => (Some(Mul), 80, 81, false),
        Tok::Slash => (Some(Div), 80, 81, false),
        Tok::Percent => (Some(Mod), 80, 81, false),

        // Exponentiation — right-associative, tighter than unary minus.
        Tok::Caret => (Some(Pow), 95, 94, false),

        _ => return None,
    };
    Some(r)
}
