//! Tokenizer for the Milkdrop HLSL dialect (run after [`crate::preprocess`]).

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Float(f64),

    // Arithmetic / assignment
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    PlusPlus,
    MinusMinus,

    // Comparison
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,

    // Logical / bitwise
    AndAnd,
    OrOr,
    Bang,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,

    // Punctuation
    Question,
    Colon,
    Dot,
    Comma,
    Semicolon,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,

    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub pos: usize,
    pub msg: String,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lex error at byte {}: {}", self.pos, self.msg)
    }
}

pub fn lex(src: &str) -> Result<Vec<Tok>, LexError> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();

    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,

            b'0'..=b'9' => {
                let (tok, ni) = lex_number(b, i)?;
                out.push(tok);
                i = ni;
            }
            b'.' if i + 1 < b.len() && b[i + 1].is_ascii_digit() => {
                let (tok, ni) = lex_number(b, i)?;
                out.push(tok);
                i = ni;
            }

            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                out.push(Tok::Ident(src[start..i].to_string()));
            }

            _ => {
                let two = if i + 1 < b.len() { &b[i..i + 2] } else { &b[i..i + 1] };
                let (tok, len): (Tok, usize) = match two {
                    b"==" => (Tok::Eq, 2),
                    b"!=" => (Tok::Ne, 2),
                    b"<=" => (Tok::Le, 2),
                    b">=" => (Tok::Ge, 2),
                    b"&&" => (Tok::AndAnd, 2),
                    b"||" => (Tok::OrOr, 2),
                    b"<<" => (Tok::Shl, 2),
                    b">>" => (Tok::Shr, 2),
                    b"++" => (Tok::PlusPlus, 2),
                    b"--" => (Tok::MinusMinus, 2),
                    b"+=" => (Tok::PlusAssign, 2),
                    b"-=" => (Tok::MinusAssign, 2),
                    b"*=" => (Tok::StarAssign, 2),
                    b"/=" => (Tok::SlashAssign, 2),
                    b"%=" => (Tok::PercentAssign, 2),
                    _ => match c {
                        b'+' => (Tok::Plus, 1),
                        b'-' => (Tok::Minus, 1),
                        b'*' => (Tok::Star, 1),
                        b'/' => (Tok::Slash, 1),
                        b'%' => (Tok::Percent, 1),
                        b'=' => (Tok::Assign, 1),
                        b'<' => (Tok::Lt, 1),
                        b'>' => (Tok::Gt, 1),
                        b'!' => (Tok::Bang, 1),
                        b'&' => (Tok::Amp, 1),
                        b'|' => (Tok::Pipe, 1),
                        b'^' => (Tok::Caret, 1),
                        b'~' => (Tok::Tilde, 1),
                        b'?' => (Tok::Question, 1),
                        b':' => (Tok::Colon, 1),
                        b'.' => (Tok::Dot, 1),
                        b',' => (Tok::Comma, 1),
                        b';' => (Tok::Semicolon, 1),
                        b'(' => (Tok::LParen, 1),
                        b')' => (Tok::RParen, 1),
                        b'{' => (Tok::LBrace, 1),
                        b'}' => (Tok::RBrace, 1),
                        b'[' => (Tok::LBracket, 1),
                        b']' => (Tok::RBracket, 1),
                        other => {
                            return Err(LexError {
                                pos: i,
                                msg: format!("unexpected character {:?}", other as char),
                            })
                        }
                    },
                };
                out.push(tok);
                i += len;
            }
        }
    }

    out.push(Tok::Eof);
    Ok(out)
}

fn lex_number(b: &[u8], start: usize) -> Result<(Tok, usize), LexError> {
    let mut i = start;

    // Hex integer.
    if b[i] == b'0' && i + 1 < b.len() && (b[i + 1] | 0x20) == b'x' {
        i += 2;
        let hs = i;
        while i < b.len() && b[i].is_ascii_hexdigit() {
            i += 1;
        }
        let s = std::str::from_utf8(&b[hs..i]).unwrap();
        let v = i64::from_str_radix(s, 16).map_err(|e| LexError { pos: start, msg: e.to_string() })?;
        return Ok((Tok::Int(v), i));
    }

    let mut is_float = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        is_float = true;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && (b[i] | 0x20) == b'e' {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            is_float = true;
            i = j;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }

    let text = std::str::from_utf8(&b[start..i]).unwrap().to_string();

    // Optional float suffix (f/F/h/H) or integer suffix (u/U/l/L).
    let mut suffixed_float = is_float;
    if i < b.len() {
        match b[i] | 0x20 {
            b'f' | b'h' => {
                suffixed_float = true;
                i += 1;
            }
            b'u' | b'l' if !is_float => {
                i += 1;
            }
            _ => {}
        }
    }

    if suffixed_float {
        let v: f64 = text.parse().map_err(|_| LexError { pos: start, msg: format!("bad float {text:?}") })?;
        Ok((Tok::Float(v), i))
    } else {
        let v: i64 = text.parse().map_err(|_| LexError { pos: start, msg: format!("bad int {text:?}") })?;
        Ok((Tok::Int(v), i))
    }
}
