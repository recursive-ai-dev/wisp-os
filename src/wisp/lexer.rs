//! Tokenizer for WISP source text. Produces a flat `Vec<Token>` up front
//! (scripts are small -- a handful of KB at most -- so there's no benefit
//! to a streaming/lazy tokenizer here, and a materialized vector makes the
//! recursive-descent parser in `parser.rs` simpler to write correctly).

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::WispError;

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
    let bytes = src.as_bytes();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut tokens = Vec::new();

    while i < bytes.len() {
        let c = bytes[i];

        match c {
            b' ' | b'\t' | b'\r' => {
                i += 1;
            }
            b'\n' => {
                line += 1;
                i += 1;
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
                    .map_err(|_| WispError::new(line, "integer literal out of range"))?;
                tokens.push(Token {
                    tok: Tok::Int(value),
                    line,
                });
            }
            b'"' => {
                i += 1; // consume opening quote
                let mut s = String::new();
                loop {
                    if i >= bytes.len() {
                        return Err(WispError::new(line, "unterminated string literal"));
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
                            ));
                        }
                        b'\\' => {
                            i += 1;
                            if i >= bytes.len() {
                                return Err(WispError::new(line, "unterminated string literal"));
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
                                    ));
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
                tokens.push(Token { tok, line });
            }
            b'+' => {
                tokens.push(Token { tok: Tok::Plus, line });
                i += 1;
            }
            b'-' => {
                tokens.push(Token { tok: Tok::Minus, line });
                i += 1;
            }
            b'*' => {
                tokens.push(Token { tok: Tok::Star, line });
                i += 1;
            }
            b'/' => {
                tokens.push(Token { tok: Tok::Slash, line });
                i += 1;
            }
            b'%' => {
                tokens.push(Token {
                    tok: Tok::Percent,
                    line,
                });
                i += 1;
            }
            b'(' => {
                tokens.push(Token {
                    tok: Tok::LParen,
                    line,
                });
                i += 1;
            }
            b')' => {
                tokens.push(Token {
                    tok: Tok::RParen,
                    line,
                });
                i += 1;
            }
            b'{' => {
                tokens.push(Token {
                    tok: Tok::LBrace,
                    line,
                });
                i += 1;
            }
            b'}' => {
                tokens.push(Token {
                    tok: Tok::RBrace,
                    line,
                });
                i += 1;
            }
            b'[' => {
                tokens.push(Token {
                    tok: Tok::LBracket,
                    line,
                });
                i += 1;
            }
            b']' => {
                tokens.push(Token {
                    tok: Tok::RBracket,
                    line,
                });
                i += 1;
            }
            b',' => {
                tokens.push(Token { tok: Tok::Comma, line });
                i += 1;
            }
            b';' => {
                tokens.push(Token { tok: Tok::Semi, line });
                i += 1;
            }
            b':' => {
                tokens.push(Token { tok: Tok::Colon, line });
                i += 1;
            }
            b'=' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::EqEq, line });
                    i += 2;
                } else {
                    tokens.push(Token { tok: Tok::Eq, line });
                    i += 1;
                }
            }
            b'!' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::NotEq, line });
                    i += 2;
                } else {
                    return Err(WispError::new(
                        line,
                        "unexpected '!' (did you mean 'not'?)",
                    ));
                }
            }
            b'<' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::Le, line });
                    i += 2;
                } else {
                    tokens.push(Token { tok: Tok::Lt, line });
                    i += 1;
                }
            }
            b'>' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push(Token { tok: Tok::Ge, line });
                    i += 2;
                } else {
                    tokens.push(Token { tok: Tok::Gt, line });
                    i += 1;
                }
            }
            other => {
                return Err(WispError::new(
                    line,
                    alloc::format!("unexpected character '{}'", other as char),
                ));
            }
        }
    }

    tokens.push(Token {
        tok: Tok::Eof,
        line,
    });
    Ok(tokens)
}
