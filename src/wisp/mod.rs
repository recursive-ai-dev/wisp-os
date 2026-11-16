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
/// Extended diagnostic context (module name, offending source line, and call
/// stack trace) recorded alongside a [`WispError`] without increasing the
/// size of `Result<Value, WispError>` on the native recursion stack.
#[derive(Debug, Clone, Default)]
pub struct ErrorDetails {
    pub module: Option<String>,
    pub source_line: Option<String>,
    pub stack_trace: alloc::vec::Vec<(String, u32)>,
}

/// A single error from any stage (lex, parse, or runtime), always carrying
/// the source line (and column when known) it happened on so a script author
/// can find the problem immediately, per the error-reporting contract in
/// `docs/WISP_LANGUAGE.md`: "wisp: line N: <message>".
#[derive(Debug, Clone)]
pub struct WispError {
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl WispError {
    pub fn new(line: u32, message: impl Into<String>) -> Self {
        Self {
            line,
            col: 0,
            message: message.into(),
        }
    }

    pub fn with_col(mut self, col: u32) -> Self {
        self.col = col;
        self
    }

    /// Render in the exact format the language spec promises.
    pub fn to_display_string(&self) -> String {
        format!("wisp: line {}: {}", self.line, self.message)
    }

    /// Render with source line, column caret, and call stack from `details`.
    pub fn to_detailed_string(&self, details: &ErrorDetails) -> String {
        let mut out = match &details.module {
            Some(module) => format!("wisp: {module}: line {}: {}", self.line, self.message),
            None => self.to_display_string(),
        };
        if let Some(src) = &details.source_line {
            let trimmed = src.trim_end();
            if !trimmed.is_empty() {
                let _ = core::fmt::Write::write_fmt(&mut out, format_args!("\n  {trimmed}"));
                if self.col > 0 {
                    let caret_pos = (self.col as usize).saturating_sub(1).min(trimmed.len());
                    let pad: String = (0..caret_pos).map(|_| ' ').collect();
                    let _ = core::fmt::Write::write_fmt(&mut out, format_args!("\n  {pad}^"));
                }
            }
        }
        for (func_name, call_line) in &details.stack_trace {
            let _ = core::fmt::Write::write_fmt(
                &mut out,
                format_args!("\n  in {func_name} (called from line {call_line})"),
            );
        }
        out
    }

    /// Errors that must never be caught by `pcall` (user Esc stop and heap exhaustion).
    pub fn is_uncatchable(&self) -> bool {
        self.message == interp::STOPPED_MESSAGE || self.message == interp::OUT_OF_MEMORY_MESSAGE
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

/// Run `source` and return detailed diagnostics (`ErrorDetails`) on failure.
pub fn run_source_detailed(
    source: &str,
    host: &mut dyn interp::Host,
) -> Result<(), (WispError, ErrorDetails)> {
    let tokens = match lexer::lex(source) {
        Ok(t) => t,
        Err(e) => {
            let mut details = ErrorDetails::default();
            attach_source_line(&mut details, e.line, source);
            return Err((e, details));
        }
    };
    let program = match parser::parse(&tokens) {
        Ok(p) => p,
        Err(e) => {
            let mut details = ErrorDetails::default();
            attach_source_line(&mut details, e.line, source);
            return Err((e, details));
        }
    };
    let mut session = interp::Session::new();
    match session.run(host, &program) {
        Ok(()) => Ok(()),
        Err(e) => {
            let mut details = session.take_error_details();
            if details.module.is_none() {
                attach_source_line(&mut details, e.line, source);
            }
            Err((e, details))
        }
    }
}

pub(crate) fn attach_source_line(details: &mut ErrorDetails, line: u32, source: &str) {
    if details.source_line.is_some() || line == 0 {
        return;
    }
    if let Some(line_str) = source.lines().nth((line - 1) as usize) {
        if line_str.len() <= 256 {
            details.source_line = Some(String::from(line_str));
        }
    }
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
    let mut tokens = lexer::lex(source)?;
    match parser::parse(&tokens) {
        Ok(program) => Ok(program),
        Err(first) => {
            if tokens.len() >= lexer::MAX_TOKENS {
                return Err(first);
            }
            if tokens.iter().rev().nth(1).is_some_and(|token| matches!(token.tok, lexer::Tok::Semi)) {
                return Err(first);
            }
            // Comments are absent from tokens: inserting before EOF works
            // even when the source ends in a comment, without copying and
            // lexing the whole source a second time.
            let eof = tokens.pop().expect("the lexer always appends EOF");
            tokens.try_reserve(2).map_err(|_| WispError::new(eof.line, interp::OUT_OF_MEMORY_MESSAGE))?;
            tokens.push(lexer::Token { tok: lexer::Tok::Semi, line: eof.line, col: eof.col });
            tokens.push(eof);
            parser::parse(&tokens).map_err(|_| first)
        }
    }
}

/// One unfinished interactive piece is bounded separately from boot apps.
pub const MAX_INTERACTIVE_SOURCE_BYTES: usize = 16 * 1024;

/// Append a prompt line atomically. Callers can discard an oversized piece
/// and continue using the same session; no partial newline is left behind.
pub fn append_interactive_line(pending: &mut String, line: &str) -> Result<(), &'static str> {
    let separator = usize::from(!pending.is_empty());
    let added = line.len().checked_add(separator).ok_or("interactive source limit is 16 KiB")?;
    if pending.len() > MAX_INTERACTIVE_SOURCE_BYTES
        || added > MAX_INTERACTIVE_SOURCE_BYTES.saturating_sub(pending.len())
    {
        return Err("interactive source limit is 16 KiB");
    }
    pending.try_reserve(added).map_err(|_| interp::OUT_OF_MEMORY_MESSAGE)?;
    if separator != 0 {
        pending.push('\n');
    }
    pending.push_str(line);
    Ok(())
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
