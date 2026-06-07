//! Tokenizer for the Milkdrop / ns-eel expression language.
//!
//! The language is C-flavored: `;`-separated statements, case-insensitive
//! identifiers, `=`/compound assignment, the usual arithmetic/logical/bitwise
//! operators, `^` for exponentiation, `[]` for megabuf addressing, and the
//! `$pi` / `$e` / `$phi` named constants.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Num(f64),
    /// Identifier, already lowercased (the language is case-insensitive).
    Ident(String),

    // Assignment
    Assign,       // =
    AddAssign,    // +=
    SubAssign,    // -=
    MulAssign,    // *=
    DivAssign,    // /=
    ModAssign,    // %=

    // Arithmetic
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret, // ^  (pow)

    // Comparison
    Eq, // ==
    Ne, // !=
    Lt,
    Gt,
    Le,
    Ge,

    // Logical / bitwise
    AndAnd, // &&
    OrOr,   // ||
    Amp,    // &
    Pipe,   // |
    Bang,   // !

    // Punctuation
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Semicolon,

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

impl std::error::Error for LexError {}

pub fn lex(src: &str) -> Result<Vec<Tok>, LexError> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();

    while i < b.len() {
        let c = b[i];
        match c {
            // Whitespace
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
            }
            // Line comment // ... and block comment /* ... */
            b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2; // skip closing */ (tolerant of EOF)
                i = i.min(b.len());
            }
            // Numbers: decimal, fractional, exponent, and 0x.. hex
            b'0'..=b'9' | b'.' if c != b'.' || i + 1 < b.len() && b[i + 1].is_ascii_digit() => {
                let (n, ni) = lex_number(b, i)?;
                out.push(Tok::Num(n));
                i = ni;
            }
            // `$pi`, `$e`, `$phi` constants — lexed as identifiers (no leading $)
            b'$' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                if j == start {
                    return Err(LexError { pos: i, msg: "stray '$'".into() });
                }
                let name = src[start..j].to_ascii_lowercase();
                out.push(Tok::Ident(name));
                i = j;
            }
            // Identifiers
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                out.push(Tok::Ident(src[start..i].to_ascii_lowercase()));
            }
            // Operators & punctuation (two-char forms first)
            _ => {
                let two = if i + 1 < b.len() { &b[i..i + 2] } else { &b[i..i + 1] };
                let (tok, len): (Tok, usize) = match two {
                    b"==" => (Tok::Eq, 2),
                    b"!=" => (Tok::Ne, 2),
                    b"<=" => (Tok::Le, 2),
                    b">=" => (Tok::Ge, 2),
                    b"&&" => (Tok::AndAnd, 2),
                    b"||" => (Tok::OrOr, 2),
                    b"+=" => (Tok::AddAssign, 2),
                    b"-=" => (Tok::SubAssign, 2),
                    b"*=" => (Tok::MulAssign, 2),
                    b"/=" => (Tok::DivAssign, 2),
                    b"%=" => (Tok::ModAssign, 2),
                    _ => match c {
                        b'+' => (Tok::Plus, 1),
                        b'-' => (Tok::Minus, 1),
                        b'*' => (Tok::Star, 1),
                        b'/' => (Tok::Slash, 1),
                        b'%' => (Tok::Percent, 1),
                        b'^' => (Tok::Caret, 1),
                        b'=' => (Tok::Assign, 1),
                        b'<' => (Tok::Lt, 1),
                        b'>' => (Tok::Gt, 1),
                        b'&' => (Tok::Amp, 1),
                        b'|' => (Tok::Pipe, 1),
                        b'!' => (Tok::Bang, 1),
                        b'(' => (Tok::LParen, 1),
                        b')' => (Tok::RParen, 1),
                        b'[' => (Tok::LBracket, 1),
                        b']' => (Tok::RBracket, 1),
                        b',' => (Tok::Comma, 1),
                        b';' => (Tok::Semicolon, 1),
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

fn lex_number(b: &[u8], start: usize) -> Result<(f64, usize), LexError> {
    let mut i = start;

    // Hex literal: 0x...
    if b[i] == b'0' && i + 1 < b.len() && (b[i + 1] | 0x20) == b'x' {
        i += 2;
        let hs = i;
        while i < b.len() && b[i].is_ascii_hexdigit() {
            i += 1;
        }
        if i == hs {
            return Err(LexError { pos: start, msg: "malformed hex literal".into() });
        }
        let s = std::str::from_utf8(&b[hs..i]).unwrap();
        let v = u64::from_str_radix(s, 16)
            .map_err(|e| LexError { pos: start, msg: e.to_string() })?;
        return Ok((v as f64, i));
    }

    // Decimal with optional fraction and exponent
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
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
            i = j;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }

    let s = std::str::from_utf8(&b[start..i]).unwrap();
    let v: f64 = s.parse().map_err(|_| LexError {
        pos: start,
        msg: format!("malformed number {s:?}"),
    })?;
    Ok((v, i))
}
