//! WISP: the small text/logic scripting language the shell's `run` command
//! executes. See `docs/WISP_LANGUAGE.md` for the full language reference --
//! this module tree is the implementation of that spec, in the same order
//! source code flows through it: `lexer` -> `parser` (building `ast`) ->
//! `interp` (using `value` and calling into `builtins`).

extern crate alloc;

use alloc::format;
use alloc::string::String;

pub mod ast;
pub mod builtins;
pub mod interp;
pub mod lexer;
pub mod parser;
pub mod value;

/// A single error from any stage (lex, parse, or runtime), always carrying
/// the source line it happened on so a script author (human or LLM) can
/// find the problem immediately, per the error-reporting contract in
/// `docs/WISP_LANGUAGE.md`: "wisp: line N: <message>".
#[derive(Debug, Clone)]
pub struct WispError {
    pub line: u32,
    pub message: String,
}

impl WispError {
    pub fn new(line: u32, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }

    /// Render in the exact format the language spec promises.
    pub fn to_display_string(&self) -> String {
        format!("wisp: line {}: {}", self.line, self.message)
    }
}

impl core::fmt::Display for WispError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.to_display_string())
    }
}

/// Lex, parse, and run a whole WISP program against `host`. This is the
/// one function the shell's `run` command needs to call; every stage's
/// error already carries a line number and formats consistently via
/// `WispError`.
pub fn run_source(source: &str, host: &mut dyn interp::Host) -> Result<(), WispError> {
    let tokens = lexer::lex(source)?;
    let program = parser::parse(&tokens)?;
    interp::Session::new().run(host, &program)
}

/// Lex and parse one piece of an interactive session (see
/// `interp::Session::run_interactive`). A piece that is a single
/// expression or statement missing only its final `;` is accepted as if
/// the `;` were there -- at a prompt, `1 + 2` means what it says, and so
/// does `let m = {"a": 1}`, which ends in a `}` that closes a map rather
/// than a block. Appending a `;` can only rescue a parse that failed at
/// the very end of the input, so any other mistake is still reported, as
/// the original parse found it.
pub fn parse_interactive(source: &str) -> Result<ast::Program, WispError> {
    let tokens = lexer::lex(source)?;
    match parser::parse(&tokens) {
        Ok(program) => Ok(program),
        Err(first) => {
            let trimmed = source.trim_end();
            if trimmed.ends_with(';') {
                return Err(first);
            }
            let mut patched = String::from(trimmed);
            patched.push(';');
            lexer::lex(&patched)
                .and_then(|tokens| parser::parse(&tokens))
                .map_err(|_| first)
        }
    }
}

/// How many more `(`, `[` and `{` than `)`, `]` and `}` `source` has,
/// ignoring any inside string literals or comments. The interactive prompt
/// keeps reading lines while this is positive, so a block can be typed over
/// several lines. Never negative: a surplus of closers is an error the
/// parser will report, not a reason to wait for more input.
pub fn open_brackets(source: &str) -> usize {
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;
    for b in source.bytes() {
        if in_comment {
            in_comment = b != b'\n';
        } else if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                // A string literal cannot span lines; the lexer reports it.
                (false, b'"') | (false, b'\n') => in_string = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'#' => in_comment = true,
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    depth
}
