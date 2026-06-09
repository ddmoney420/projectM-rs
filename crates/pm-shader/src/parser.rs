//! Recursive-descent parser for the Milkdrop HLSL dialect.

use crate::ast::*;
use crate::lexer::{lex, LexError, Tok};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    Lex(LexError),
    Unexpected { got: Tok, expected: String },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Lex(e) => write!(f, "{e}"),
            ParseError::Unexpected { got, expected } => {
                write!(f, "unexpected {got}, expected {expected}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError::Lex(e)
    }
}

/// Parse a full translation unit (after preprocessing).
pub fn parse(src: &str) -> Result<Vec<Item>, ParseError> {
    let toks = lex(src)?;
    let mut p = Parser { toks, pos: 0 };
    let mut items = Vec::new();
    while !p.at(&Tok::Eof) {
        p.parse_item(&mut items)?;
    }
    Ok(items)
}

fn type_from_ident(s: &str) -> Option<Type> {
    use Type::*;
    Some(match s {
        "void" => Void,
        // `double`/`half` are aliased to `float`: Milkdrop/projectM GPU shader
        // paths treat all of these as float precision.
        "float" | "half" | "double" | "float1" | "half1" | "double1" => Float,
        "int" | "uint" | "dword" | "int1" | "uint1" => Int,
        "bool" | "bool1" => Bool,
        "float2" | "half2" | "double2" => Float2,
        "float3" | "half3" | "double3" => Float3,
        "float4" | "half4" | "double4" => Float4,
        "int2" | "uint2" => Int2,
        "int3" | "uint3" => Int3,
        "int4" | "uint4" => Int4,
        "bool2" => Bool2,
        "bool3" => Bool3,
        "bool4" => Bool4,
        "float2x2" => Mat2,
        "float3x3" => Mat3,
        "float4x4" => Mat4,
        "float4x3" => Mat4x3,
        "float3x4" => Mat3x4,
        "sampler2D" | "sampler" => Sampler2D,
        "sampler3D" => Sampler3D,
        _ => return None,
    })
}

const TYPE_QUALIFIERS: &[&str] = &["uniform", "const", "static", "inline", "row_major", "column_major"];

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos]
    }
    fn peek_at(&self, n: usize) -> &Tok {
        self.toks.get(self.pos + n).unwrap_or(&Tok::Eof)
    }
    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].clone();
        self.pos += 1;
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: &Tok, what: &str) -> Result<(), ParseError> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(ParseError::Unexpected { got: self.peek().clone(), expected: what.to_string() })
        }
    }
    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.bump() {
            Tok::Ident(s) => Ok(s),
            got => Err(ParseError::Unexpected { got, expected: "identifier".into() }),
        }
    }

    /// True if the current token is a known type keyword.
    fn peek_type(&self) -> Option<Type> {
        if let Tok::Ident(s) = self.peek() {
            type_from_ident(s)
        } else {
            None
        }
    }

    /// True if a (possibly qualifier-prefixed) declaration starts here:
    /// `[const|static|...] <type> <ident>`.
    fn peek_decl(&self) -> bool {
        let mut q = 0;
        while let Tok::Ident(s) = self.peek_at(q) {
            if TYPE_QUALIFIERS.contains(&s.as_str()) {
                q += 1;
            } else {
                break;
            }
        }
        matches!(self.peek_at(q), Tok::Ident(s) if type_from_ident(s).is_some())
            && matches!(self.peek_at(q + 1), Tok::Ident(_))
    }

    /// Skip leading type qualifiers (`const`, `static`, …) on a declaration.
    fn skip_qualifiers(&mut self) {
        while matches!(self.peek(), Tok::Ident(s) if TYPE_QUALIFIERS.contains(&s.as_str())) {
            self.pos += 1;
        }
    }

    // ----------------------------------------------------------- items -------

    fn parse_item(&mut self, items: &mut Vec<Item>) -> Result<(), ParseError> {
        let mut uniform = false;
        // Skip/record leading qualifiers.
        while let Tok::Ident(s) = self.peek() {
            if TYPE_QUALIFIERS.contains(&s.as_str()) {
                if s == "uniform" {
                    uniform = true;
                }
                self.pos += 1;
            } else {
                break;
            }
        }

        let ty = self
            .peek_type()
            .ok_or_else(|| ParseError::Unexpected { got: self.peek().clone(), expected: "a type".into() })?;
        self.pos += 1;

        let name = self.expect_ident()?;

        // Function?
        if self.at(&Tok::LParen) {
            let params = self.parse_params()?;
            let body = self.parse_block_stmts()?;
            items.push(Item::Function(Function { ret: ty, name, params, body }));
            return Ok(());
        }

        // Sampler declaration.
        if matches!(ty, Type::Sampler2D | Type::Sampler3D) {
            self.expect(&Tok::Semicolon, "';'")?;
            items.push(Item::Sampler { ty, name });
            return Ok(());
        }

        // Global variable(s), possibly comma-separated: `uniform float4 a, b, c;`.
        let mut first_name = Some(name);
        loop {
            let var_name = match first_name.take() {
                Some(n) => n,
                None => self.expect_ident()?,
            };
            let semantic = self.parse_semantic()?;
            let init = if self.eat(&Tok::Assign) { Some(self.parse_expr()?) } else { None };
            items.push(Item::Global { uniform, ty, name: var_name, semantic, init });

            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::Semicolon, "';'")?;
        Ok(())
    }

    fn parse_semantic(&mut self) -> Result<Option<String>, ParseError> {
        if self.eat(&Tok::Colon) {
            Ok(Some(self.expect_ident()?))
        } else {
            Ok(None)
        }
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
        self.expect(&Tok::LParen, "'('")?;
        let mut params = Vec::new();
        if self.eat(&Tok::RParen) {
            return Ok(params);
        }
        loop {
            // Parameter qualifier.
            let mut qualifier = ParamQual::In;
            while let Tok::Ident(s) = self.peek() {
                match s.as_str() {
                    "out" => {
                        qualifier = ParamQual::Out;
                        self.pos += 1;
                    }
                    "inout" => {
                        qualifier = ParamQual::InOut;
                        self.pos += 1;
                    }
                    "in" | "const" | "uniform" => {
                        self.pos += 1;
                    }
                    _ => break,
                }
            }
            let ty = self
                .peek_type()
                .ok_or_else(|| ParseError::Unexpected { got: self.peek().clone(), expected: "parameter type".into() })?;
            self.pos += 1;
            let name = self.expect_ident()?;
            let semantic = self.parse_semantic()?;
            params.push(Param { qualifier, ty, name, semantic });

            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen, "')'")?;
        Ok(params)
    }

    // ------------------------------------------------------- statements ------

    fn parse_block_stmts(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect(&Tok::LBrace, "'{'")?;
        let mut stmts = Vec::new();
        while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
            self.parse_statement(&mut stmts)?;
        }
        self.expect(&Tok::RBrace, "'}'")?;
        Ok(stmts)
    }

    fn parse_block(&mut self) -> Result<Stmt, ParseError> {
        Ok(Stmt::Block(self.parse_block_stmts()?))
    }

    /// Parse one statement which can become a single statement nested under
    /// control flow (a brace block stays a block; otherwise wrap if needed).
    fn parse_substatement(&mut self) -> Result<Stmt, ParseError> {
        if self.at(&Tok::LBrace) {
            return self.parse_block();
        }
        let mut v = Vec::new();
        self.parse_statement(&mut v)?;
        if v.len() == 1 {
            Ok(v.pop().unwrap())
        } else {
            Ok(Stmt::Block(v))
        }
    }

    fn parse_statement(&mut self, out: &mut Vec<Stmt>) -> Result<(), ParseError> {
        // Empty statement.
        if self.eat(&Tok::Semicolon) {
            return Ok(());
        }
        if self.at(&Tok::LBrace) {
            out.push(self.parse_block()?);
            return Ok(());
        }

        if let Tok::Ident(s) = self.peek() {
            match s.as_str() {
                "if" => return self.parse_if(out),
                "for" => return self.parse_for(out),
                "while" => return self.parse_while(out),
                "return" => {
                    self.pos += 1;
                    let value = if self.at(&Tok::Semicolon) { None } else { Some(self.parse_expr()?) };
                    self.expect(&Tok::Semicolon, "';'")?;
                    out.push(Stmt::Return(value));
                    return Ok(());
                }
                _ => {}
            }
        }

        // Declaration: an optional qualifier + type keyword + identifier name.
        if self.peek_decl() {
            return self.parse_decl(out);
        }

        // Expression statement. A top-level comma operator (`a, b, c;`) becomes
        // a sequence of expression statements (only side effects matter here).
        out.push(Stmt::Expr(self.parse_expr()?));
        while self.eat(&Tok::Comma) {
            out.push(Stmt::Expr(self.parse_expr()?));
        }
        self.expect(&Tok::Semicolon, "';'")?;
        Ok(())
    }

    fn parse_decl(&mut self, out: &mut Vec<Stmt>) -> Result<(), ParseError> {
        self.skip_qualifiers();
        let ty = self.peek_type().unwrap();
        self.pos += 1;
        loop {
            let name = self.expect_ident()?;
            // Ignore array suffix dimensions for now (rare in preset bodies).
            let init = if self.eat(&Tok::Assign) { Some(self.parse_expr()?) } else { None };
            out.push(Stmt::Decl { ty, name, init });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::Semicolon, "';'")?;
        Ok(())
    }

    fn parse_if(&mut self, out: &mut Vec<Stmt>) -> Result<(), ParseError> {
        self.pos += 1; // 'if'
        self.expect(&Tok::LParen, "'('")?;
        let cond = self.parse_expr()?;
        self.expect(&Tok::RParen, "')'")?;
        let then = Box::new(self.parse_substatement()?);
        let els = if matches!(self.peek(), Tok::Ident(s) if s == "else") {
            self.pos += 1;
            Some(Box::new(self.parse_substatement()?))
        } else {
            None
        };
        out.push(Stmt::If(cond, then, els));
        Ok(())
    }

    fn parse_while(&mut self, out: &mut Vec<Stmt>) -> Result<(), ParseError> {
        self.pos += 1; // 'while'
        self.expect(&Tok::LParen, "'('")?;
        let cond = self.parse_expr()?;
        self.expect(&Tok::RParen, "')'")?;
        let body = Box::new(self.parse_substatement()?);
        out.push(Stmt::While(cond, body));
        Ok(())
    }

    fn parse_for(&mut self, out: &mut Vec<Stmt>) -> Result<(), ParseError> {
        self.pos += 1; // 'for'
        self.expect(&Tok::LParen, "'('")?;

        // init
        let init = if self.at(&Tok::Semicolon) {
            self.pos += 1;
            None
        } else if self.peek_decl() {
            let mut v = Vec::new();
            self.parse_decl(&mut v)?; // consumes trailing ';'
            Some(Box::new(if v.len() == 1 { v.pop().unwrap() } else { Stmt::Block(v) }))
        } else {
            let e = self.parse_expr()?;
            self.expect(&Tok::Semicolon, "';'")?;
            Some(Box::new(Stmt::Expr(e)))
        };

        // condition
        let cond = if self.at(&Tok::Semicolon) { None } else { Some(self.parse_expr()?) };
        self.expect(&Tok::Semicolon, "';'")?;

        // update
        let update = if self.at(&Tok::RParen) { None } else { Some(self.parse_expr()?) };
        self.expect(&Tok::RParen, "')'")?;

        let body = Box::new(self.parse_substatement()?);
        out.push(Stmt::For(init, cond, update, body));
        Ok(())
    }

    // ------------------------------------------------------ expressions ------

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Result<Expr, ParseError> {
        let lhs = self.parse_ternary()?;
        let op = match self.peek() {
            Tok::Assign => AssignOp::Assign,
            Tok::PlusAssign => AssignOp::Add,
            Tok::MinusAssign => AssignOp::Sub,
            Tok::StarAssign => AssignOp::Mul,
            Tok::SlashAssign => AssignOp::Div,
            Tok::PercentAssign => AssignOp::Mod,
            _ => return Ok(lhs),
        };
        self.pos += 1;
        let rhs = self.parse_assignment()?;
        Ok(Expr::Assign(op, Box::new(lhs), Box::new(rhs)))
    }

    fn parse_ternary(&mut self) -> Result<Expr, ParseError> {
        let cond = self.parse_binary(0)?;
        if self.eat(&Tok::Question) {
            let then = self.parse_assignment()?;
            self.expect(&Tok::Colon, "':'")?;
            let els = self.parse_assignment()?;
            Ok(Expr::Ternary(Box::new(cond), Box::new(then), Box::new(els)))
        } else {
            Ok(cond)
        }
    }

    fn parse_binary(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_unary()?;
        while let Some((op, l_bp, r_bp)) = binop_binding(self.peek()) {
            if l_bp < min_bp {
                break;
            }
            self.pos += 1;
            let rhs = self.parse_binary(r_bp)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Tok::Minus => {
                self.pos += 1;
                Ok(Expr::Unary(UnOp::Neg, Box::new(self.parse_unary()?)))
            }
            Tok::Plus => {
                self.pos += 1;
                self.parse_unary()
            }
            Tok::Bang => {
                self.pos += 1;
                Ok(Expr::Unary(UnOp::Not, Box::new(self.parse_unary()?)))
            }
            Tok::Tilde => {
                self.pos += 1;
                Ok(Expr::Unary(UnOp::BitNot, Box::new(self.parse_unary()?)))
            }
            Tok::PlusPlus => {
                self.pos += 1;
                Ok(Expr::PreInc(Box::new(self.parse_unary()?)))
            }
            Tok::MinusMinus => {
                self.pos += 1;
                Ok(Expr::PreDec(Box::new(self.parse_unary()?)))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.parse_primary()?;
        loop {
            match self.peek() {
                Tok::Dot => {
                    self.pos += 1;
                    let field = self.expect_ident()?;
                    e = Expr::Member(Box::new(e), field);
                }
                Tok::LBracket => {
                    self.pos += 1;
                    let idx = self.parse_expr()?;
                    self.expect(&Tok::RBracket, "']'")?;
                    e = Expr::Index(Box::new(e), Box::new(idx));
                }
                Tok::LParen => {
                    // Function call on a bare identifier.
                    if let Expr::Ident(name) = e {
                        let args = self.parse_call_args()?;
                        e = Expr::Call(name, args);
                    } else {
                        return Ok(e);
                    }
                }
                Tok::PlusPlus => {
                    self.pos += 1;
                    e = Expr::PostInc(Box::new(e));
                }
                Tok::MinusMinus => {
                    self.pos += 1;
                    e = Expr::PostDec(Box::new(e));
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn parse_call_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        self.expect(&Tok::LParen, "'('")?;
        let mut args = Vec::new();
        if self.eat(&Tok::RParen) {
            return Ok(args);
        }
        loop {
            args.push(self.parse_assignment()?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RParen, "')'")?;
        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek().clone() {
            Tok::Int(v) => {
                self.pos += 1;
                Ok(Expr::IntLit(v))
            }
            Tok::Float(v) => {
                self.pos += 1;
                Ok(Expr::FloatLit(v))
            }
            Tok::Ident(s) => {
                if s == "true" {
                    self.pos += 1;
                    return Ok(Expr::BoolLit(true));
                }
                if s == "false" {
                    self.pos += 1;
                    return Ok(Expr::BoolLit(false));
                }
                // Type constructor: `float3( ... )`.
                if let Some(ty) = type_from_ident(&s) {
                    if matches!(self.peek_at(1), Tok::LParen) {
                        self.pos += 1; // type name
                        let args = self.parse_call_args()?;
                        return Ok(Expr::Construct(ty, args));
                    }
                }
                self.pos += 1;
                Ok(Expr::Ident(s))
            }
            Tok::LParen => {
                // Cast: `( TYPE ) operand`.
                if let Tok::Ident(s) = self.peek_at(1) {
                    if let Some(ty) = type_from_ident(s) {
                        if matches!(self.peek_at(2), Tok::RParen) {
                            self.pos += 3; // '(' type ')'
                            let operand = self.parse_unary()?;
                            return Ok(Expr::Construct(ty, vec![operand]));
                        }
                    }
                }
                self.pos += 1; // '('
                let e = self.parse_expr()?;
                self.expect(&Tok::RParen, "')'")?;
                Ok(e)
            }
            got => Err(ParseError::Unexpected { got, expected: "expression".into() }),
        }
    }
}

/// `(op, left_bp, right_bp)` for binary operators. Higher bp binds tighter.
fn binop_binding(t: &Tok) -> Option<(BinOp, u8, u8)> {
    use BinOp::*;
    Some(match t {
        Tok::OrOr => (Or, 1, 2),
        Tok::AndAnd => (And, 3, 4),
        Tok::Pipe => (BitOr, 5, 6),
        Tok::Caret => (BitXor, 7, 8),
        Tok::Amp => (BitAnd, 9, 10),
        Tok::Eq => (Eq, 11, 12),
        Tok::Ne => (Ne, 11, 12),
        Tok::Lt => (Lt, 13, 14),
        Tok::Gt => (Gt, 13, 14),
        Tok::Le => (Le, 13, 14),
        Tok::Ge => (Ge, 13, 14),
        Tok::Shl => (Shl, 15, 16),
        Tok::Shr => (Shr, 15, 16),
        Tok::Plus => (Add, 17, 18),
        Tok::Minus => (Sub, 17, 18),
        Tok::Star => (Mul, 19, 20),
        Tok::Slash => (Div, 19, 20),
        Tok::Percent => (Mod, 19, 20),
        _ => return None,
    })
}
