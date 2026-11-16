//! Tokenizer for WISP source text. Produces a flat `Vec<Token>` up front
//! (scripts are small -- a handful of KB at most -- so there's no benefit
//! to a streaming/lazy tokenizer here, and a materialized vector makes the
//! recursive-descent parser in `parser.rs` simpler to write correctly).

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::WispError;

/// Includes the final EOF token. A token occupies 24 bytes on x86_64,
/// so even dense source uses at most 768 KiB for the token vector.
pub const MAX_TOKENS: usize = 32 * 1024;
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Int(i64),
    Str(Rc<String>),
    Ident(Rc<String>),

    True,
    False,
    Nil,
    Let,
    If,
    Elif,
    Else,
    While,
    For,
    In,
    Fn,
    Return,
    Break,
    Continue,
    And,
    Or,
    Not,

    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,

    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    pub col: u32,
}

fn keyword(ident: &str) -> Option<Tok> {
    Some(match ident {
        "true" => Tok::True,
        "false" => Tok::False,
        "nil" => Tok::Nil,
        "let" => Tok::Let,
        "if" => Tok::If,
        "elif" => Tok::Elif,
        "else" => Tok::Else,
        "while" => Tok::While,
        "for" => Tok::For,
        "in" => Tok::In,
        "fn" => Tok::Fn,
        "return" => Tok::Return,
        "break" => Tok::Break,
        "continue" => Tok::Continue,
        "and" => Tok::And,
        "or" => Tok::Or,
        "not" => Tok::Not,
        _ => return None,
    })
}

pub fn lex(src: &str) -> Result<Vec<Token>, WispError> {
    if src.len() > MAX_SOURCE_BYTES {
        return Err(WispError::new(1, "source limit is 256 KiB"));
    }
    let bytes = src.as_bytes();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut line_start = 0usize;
    let mut tokens = Vec::new();

    // Skip UTF-8 BOM if present (0xEF 0xBB 0xBF)
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        i = 3;
        line_start = 3;
    }

    while i < bytes.len() {
        let c = bytes[i];
        let col = (i.saturating_sub(line_start) + 1) as u32;

        if !matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'#') {
            if tokens.len() >= MAX_TOKENS - 1 {
                return Err(WispError::new(line, "program has too many tokens (limit is 32768 including EOF)").with_col(col));
            }
            // Keep room for both this token and EOF. Vec's geometric
            // growth stays within MAX_TOKENS (a power of two).
            tokens.try_reserve(2).map_err(|_| WispError::new(line, super::interp::OUT_OF_MEMORY_MESSAGE))?;
        }

        match c {
            b' ' | b'\t' | b'\r' => {
                i += 1;
            }
            b'\n' => {
                line += 1;
                i += 1;
                line_start = i;
            }
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'0'..=b'9' => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                // SAFETY/correctness: the slice [start..i] is, by
                // construction, entirely ASCII digits, so it is valid
                // UTF-8 and always parses as an i64 unless it's too long
                // to fit -- which we report as a clean lex error instead
                // of panicking on overflow.
                let text = core::str::from_utf8(&bytes[start..i]).unwrap();
                let value: i64 = text
                    .parse()
                    .map_err(|_| WispError::new(line, "integer literal out of range").with_col(col))?;
                tokens.push(Token {
                    tok: Tok::Int(value),
                    line,
                    col,
                });
            }
            b'"' => {
                i += 1; // consume opening quote
                let mut s = String::new();
                loop {
                    if i >= bytes.len() {
                        return Err(WispError::new(line, "unterminated string literal").with_col(col));
                    }
                    match bytes[i] {
                        b'"' => {
                            i += 1;
                            break;
                        }
                        b'\n' => {
                            return Err(WispError::new(
                                line,
                                "unterminated string literal (newline before closing '\"')",
                            ).with_col(col));
                        }
                        b'\\' => {
                            i += 1;
                            if i >= bytes.len() {
                                return Err(WispError::new(line, "unterminated string literal").with_col(col));
                            }
                            let escaped = match bytes[i] {
                                b'"' => '"',
                                b'\\' => '\\',
                                b'n' => '\n',
                                b't' => '\t',
                                other => {
                                    return Err(WispError::new(
                                        line,
                                        alloc::format!(
                                            "unknown escape sequence '\\{}'",
                                            other as char
                                        ),
                                    ).with_col((i.saturating_sub(line_start)) as u32));
                                }
                            };
                            s.push(escaped);
                            i += 1;
                        }
                        b if b.is_ascii() => {
                            s.push(b as char);
                            i += 1;
                        }
                        _ => {
                            // A non-ASCII character: decode the whole UTF-8
                            // sequence rather than casting each byte to a
                            // `char` (which would turn `é`, bytes C3 A9,
                            // into the two characters `Ã©`). `src` is a
                            // `&str` and every earlier byte consumed was
                            // either ASCII or a whole character, so `i` is
                            // always on a character boundary here.
                            let ch = src[i..]
                                .chars()
                                .next()
                                .expect("i is a char boundary inside a non-empty &str");
                            s.push(ch);
                            i += ch.len_utf8();
                        }
                    }
                }
                tokens.push(Token {
                    tok: Tok::Str(Rc::new(s)),
                    line,
                    col,
                });
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                {
                    i += 1;
                }
                let text = core::str::from_utf8(&bytes[start..i]).unwrap();
                let tok = keyword(text).unwrap_or_else(|| Tok::Ident(Rc::new(text.into())));
                tokens.push(Token { tok, line, col });
            }
            b'+' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::PlusEq, line, col });
                    i += 2;
                } else {
                    tokens.push(Token { tok: Tok::Plus, line, col });
                    i += 1;
                }
            }
            b'-' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::MinusEq, line, col });
                    i += 2;
                } else {
                    tokens.push(Token { tok: Tok::Minus, line, col });
                    i += 1;
                }
            }
            b'*' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::StarEq, line, col });
                    i += 2;
                } else {
                    tokens.push(Token { tok: Tok::Star, line, col });
                    i += 1;
                }
            }
            b'/' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::SlashEq, line, col });
                    i += 2;
                } else {
                    tokens.push(Token { tok: Tok::Slash, line, col });
                    i += 1;
                }
            }
            b'%' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::PercentEq, line, col });
                    i += 2;
                } else {
                    tokens.push(Token {
                        tok: Tok::Percent,
                        line,
                        col,
                    });
                    i += 1;
                }
            }
            b'(' => {
                tokens.push(Token {
                    tok: Tok::LParen,
                    line,
                    col,
                });
                i += 1;
            }
            b')' => {
                tokens.push(Token {
                    tok: Tok::RParen,
                    line,
                    col,
                });
                i += 1;
            }
            b'{' => {
                tokens.push(Token {
                    tok: Tok::LBrace,
                    line,
                    col,
                });
                i += 1;
            }
            b'}' => {
                tokens.push(Token {
                    tok: Tok::RBrace,
                    line,
                    col,
                });
                i += 1;
            }
            b'[' => {
                tokens.push(Token {
                    tok: Tok::LBracket,
                    line,
                    col,
                });
                i += 1;
            }
            b']' => {
                tokens.push(Token {
                    tok: Tok::RBracket,
                    line,
                    col,
                });
                i += 1;
            }
            b',' => {
                tokens.push(Token { tok: Tok::Comma, line, col });
                i += 1;
            }
            b';' => {
                tokens.push(Token { tok: Tok::Semi, line, col });
                i += 1;
            }
            b':' => {
                tokens.push(Token { tok: Tok::Colon, line, col });
                i += 1;
            }
            b'=' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::EqEq, line, col });
                    i += 2;
                } else {
                    tokens.push(Token { tok: Tok::Eq, line, col });
                    i += 1;
                }
            }
            b'!' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::NotEq, line, col });
                    i += 2;
                } else {
                    return Err(WispError::new(
                        line,
                        "unexpected '!' (did you mean 'not'?)",
                    ).with_col(col));
                }
            }
            b'<' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::Le, line, col });
                    i += 2;
                } else {
                    tokens.push(Token { tok: Tok::Lt, line, col });
                    i += 1;
                }
            }
            b'>' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::Ge, line, col });
                    i += 2;
                } else {
                    tokens.push(Token { tok: Tok::Gt, line, col });
                    i += 1;
                }
            }
            other => {
                return Err(WispError::new(
                    line,
                    alloc::format!("unexpected character '{}'", other as char),
                ).with_col(col));
            }
        }
    }

    let eof_col = (i.saturating_sub(line_start) + 1) as u32;
    tokens.try_reserve(1).map_err(|_| WispError::new(line, super::interp::OUT_OF_MEMORY_MESSAGE))?;
    tokens.push(Token {
        tok: Tok::Eof,
        line,
        col: eof_col,
    });
    Ok(tokens)
}
