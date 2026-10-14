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
    interp::Interp::new(host).run(&program)
}
