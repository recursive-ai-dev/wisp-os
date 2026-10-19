//! Tree-walking interpreter: `ast::Program` -> side effects (via `Host`) +
//! a `Result<(), WispError>`.
//!
//! Scoping model: a single global scope, plus a *stack* of block-local
//! scopes that grows and shrinks as execution enters and leaves `if`/
//! `while` bodies and function calls:
//!
//! - A function call starts a brand-new stack containing exactly one
//!   frame (the parameters) -- it can never see the caller's locals, so
//!   there are no closures.
//! - Entering an `if`-branch body or a `while`-loop body pushes a fresh,
//!   empty frame onto the *current* stack, and pops it back off on the
//!   way out (however the block ended: falling off the end, `break`,
//!   `continue`, or `return`). This means a `let` inside a loop body
//!   declares a brand-new variable every iteration, rather than
//!   colliding with the previous iteration's now-discarded one.
//! - Top-level code (outside any function) runs against an initially
//!   empty stack, so a top-level `let` not inside any block still
//!   becomes a true global -- but a top-level `if`/`while` block gets
//!   its own scope frame exactly like anywhere else.
//! - Variable lookup/assignment searches the stack innermost-frame-first,
//!   then falls back to the globals.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use super::ast::{BinaryOp, Expr, FuncDecl, Program, Stmt, TopItem, UnaryOp};
use super::builtins;
use super::value::{self, List, ListRegistry, Value};
use super::WispError;

/// Recursion guard: a user function calling itself (directly or via a
/// cycle) more than this many times deep is reported as a clean runtime
/// error instead of exhausting this interpreter's real call stack (which,
/// since this is a tree-walker, backs every WISP call with a real Rust
/// stack frame) and taking the whole firmware shell down with it.
const MAX_CALL_DEPTH: usize = 256;

/// Budget for *all* native recursion the tree-walker performs -- every
/// nested statement, expression, and function call counts one unit. The
/// call-depth limit above only counts WISP function calls; on its own it
/// would let 256 calls each sit under a deeply nested expression, or a
/// single call-free expression nest arbitrarily deep, and run the kernel
/// stack out. The parser separately caps one expression/block's syntactic
/// depth (`parser::MAX_SYNTAX_DEPTH`); this caps the product.
///
/// Sized from the release build's actual frames on the UEFI target (see
/// `CHANGES.md`): the costliest chain of native frames between two
/// consecutive units is ~750 bytes (a nested `if`/`while` body), so 1024
/// units stay under ~770 KiB of the 1 MiB mainline stack (`gdt.rs`). A
/// simple recursive function uses 4-5 units per call, so plain recursion
/// still reaches 200-256 calls deep.
pub const MAX_NESTING: usize = 1024;

/// Message for the error raised when the host reports the heap is down to
/// its reserve. The script stops; anything it built is reclaimed.
pub const OUT_OF_MEMORY_MESSAGE: &str = "out of memory (the script used up the heap reserve and was stopped)";

/// A stack of block-local scope frames, innermost last. See the module
/// doc comment above for the full scoping model.
type ScopeStack = Vec<BTreeMap<String, Value>>;

/// Everything the interpreter needs from the outside world. The shell
/// implements this against the real screen/keyboard/timer; keeping it as
/// a trait keeps the language core decoupled from `uefi` types.
pub trait Host {
    fn print(&mut self, s: &str);
    /// Print `prompt` (no trailing newline), then block for a line of
    /// input (echoed, with backspace support) and return it once Enter is
    /// pressed.
    fn input(&mut self, prompt: &str) -> String;
    fn clear(&mut self);
    fn sleep_ms(&mut self, ms: u64);
    /// A pseudo-random `u64`, uniformly distributed, from whatever PRNG
    /// the host maintains.
    fn rand_u64(&mut self) -> u64;
    /// Persist `value` under `key`, surviving a reboot -- backed by
    /// `save_format.rs` over the AHCI save disk (Phase 2, Part B).
    /// Replaces any value already saved under `key`, crash-atomically;
    /// saving `Value::Nil` deletes the key. `Err` covers both "no save
    /// disk available this boot" and every `save_format::save` failure
    /// (key too long, value too large, store full).
    fn save(&mut self, key: &str, value: &Value) -> Result<(), &'static str>;
    /// Look up a value previously saved under `key`. `Ok(None)` means
    /// `key` was never saved -- a normal, first-class outcome, not an
    /// error (see `builtins::builtin_load`'s doc comment for how the
    /// `load()` builtin surfaces this to a WISP script). `Err` covers "no
    /// save disk available this boot" and real on-disk corruption
    /// (`save_format::load`'s checksum/format checks).
    fn load(&mut self, key: &str) -> Result<Option<Value>, &'static str>;

    /// Whether the user has asked the running program to stop (the Esc
    /// key). Polled by the interpreter before every `while` iteration and
    /// every user-function call -- the only two constructs that can make a
    /// WISP program run unboundedly long -- and by the blocking builtins
    /// (`input`, `sleep_ms`) after they return. Must be cheap: it is on
    /// the hot path of every loop.
    fn stop_requested(&mut self) -> bool;

    /// Whether free heap has fallen to the host's safety reserve. Polled
    /// at the same points as `stop_requested`; a `true` stops the script
    /// with [`OUT_OF_MEMORY_MESSAGE`] while there is still memory left to
    /// report it and clean up, rather than letting an infallible
    /// allocation exhaust the heap and halt the kernel. Defaults to
    /// "never", for hosts without a shared fixed-size heap.
    fn memory_low(&mut self) -> bool {
        false
    }

    /// Milliseconds since boot, from the kernel's monotonic clock. Backs
    /// the `time_ms()` builtin.
    fn time_ms(&mut self) -> u64;

    // ---- WISP v2: pixel/graphics primitives ----------------------------
    //
    // Backed by `gfx::Console`'s raw-framebuffer methods of the same
    // shape; see that module for the bounds-safety reasoning (every
    // coordinate here is fully untrusted, script-controlled input) and
    // `docs/WISP_GRAPHICS.md` for the language-level contract these
    // implement. A color is always a plain `u32` packed as `0xRRGGBB` --
    // see `builtins::builtin_rgb` -- never a compound type, keeping this
    // trait's signatures scalar-primitives-only, the same as every
    // existing method above.

    /// Pixel width of the graphics framebuffer this host draws into.
    /// Fixed for the life of the program (the display mode is set once at
    /// boot and never changed afterward), so a script can read it once
    /// and reuse it, e.g. to center something with `screen_width() / 2`.
    fn screen_width(&self) -> usize;

    /// Pixel height of the graphics framebuffer. See `screen_width`.
    fn screen_height(&self) -> usize;

    /// Set one pixel to `color` (packed `0xRRGGBB`). `(x, y)` outside the
    /// visible framebuffer is silently ignored -- see
    /// `docs/WISP_GRAPHICS.md`'s clipping contract.
    fn plot(&mut self, x: i64, y: i64, color: u32);

    /// Fill an axis-aligned rectangle of `color`, top-left corner
    /// `(x, y)`, `w` wide and `h` tall, clipped to the visible
    /// framebuffer. `w`/`h` are `usize`, not `i64`, because
    /// `builtins::builtin_rect` has already rejected a negative size with
    /// a WISP-level error before this is ever called.
    fn draw_rect(&mut self, x: i64, y: i64, w: usize, h: usize, color: u32);

    /// Draw a straight line of `color` from `(x1, y1)` to `(x2, y2)`
    /// inclusive, clipped per-pixel to the visible framebuffer.
    /// `builtins::builtin_line` has already rejected wildly-out-of-range
    /// coordinates with a WISP-level error before this is ever called.
    fn draw_line(&mut self, x1: i64, y1: i64, x2: i64, y2: i64, color: u32);

    /// Fill the entire visible framebuffer with `color`. Must not move or
    /// otherwise affect the text cursor -- see `docs/WISP_GRAPHICS.md`'s
    /// note on how text and graphics share one framebuffer.
    fn fill_screen(&mut self, color: u32);
}

/// The runtime error a program stopped with Esc ends with. Reported in the
/// normal `wisp: line N: ...` form, at the loop or call it was stopped in,
/// which tells the author where a runaway program was spending its time.
pub const STOPPED_MESSAGE: &str = "stopped (Esc pressed)";

/// What a statement (or a block of them) resolved to, for propagating
/// `break`/`continue`/`return` up through nested blocks without
/// exceptions.
enum Flow {
    Normal,
    /// Carries the `break`/`continue` statement's own source line, so an
    /// "outside of a loop" error (if it turns out there's no enclosing
    /// loop to catch it) points at the statement itself rather than at
    /// whatever call frame happens to be unwinding when the error is
    /// finally raised.
    Break(u32),
    Continue(u32),
    Return(Value),
}

pub struct Interp<'h> {
    host: &'h mut dyn Host,
    globals: BTreeMap<String, Value>,
    functions: BTreeMap<String, Rc<FuncDecl>>,
    call_depth: usize,
    /// See [`MAX_NESTING`].
    nesting: usize,
    /// Every list this run creates; emptied when the interpreter is
    /// dropped, which is what frees cyclic lists. See [`ListRegistry`].
    lists: ListRegistry,
}

impl<'h> Interp<'h> {
    pub fn new(host: &'h mut dyn Host) -> Self {
        Self {
            host,
            globals: BTreeMap::new(),
            functions: BTreeMap::new(),
            call_depth: 0,
            nesting: 0,
            lists: ListRegistry::new(),
        }
    }

    /// Run a whole program: register every `fn` first (so forward
    /// references work regardless of source order), then execute the
    /// top-level statements in their original order.
    pub fn run(&mut self, program: &Program) -> Result<(), WispError> {
        for item in &program.items {
            if let TopItem::Func(f) = item {
                if builtins::is_builtin(&f.name) {
                    return Err(WispError::new(
                        f.line,
                        format!("'{}' is a built-in function and cannot be redefined", f.name),
                    ));
                }
                if self.functions.contains_key(f.name.as_str()) {
                    return Err(WispError::new(
                        f.line,
                        format!("function '{}' is already defined", f.name),
                    ));
                }
                self.functions.insert((*f.name).clone(), f.clone());
            }
        }

        // Top-level code starts with an empty scope stack: a bare `let`
        // here writes straight into `self.globals`, while a top-level
        // `if`/`while` block still gets its own pushed/popped frame.
        let mut stack: ScopeStack = Vec::new();

        for item in &program.items {
            if let TopItem::Stmt(s) = item {
                match self.exec_stmt(s, &mut stack)? {
                    Flow::Normal => {}
                    // A bare top-level `return;` is a documented way to
                    // end the script early (e.g. after a "game over"
                    // message) -- there's no function to return *from*,
                    // but "stop running the rest of the program" is an
                    // unambiguous, useful reading of it.
                    Flow::Return(_) => return Ok(()),
                    Flow::Break(line) => {
                        return Err(WispError::new(line, "'break' outside of a loop"))
                    }
                    Flow::Continue(line) => {
                        return Err(WispError::new(line, "'continue' outside of a loop"))
                    }
                }
            }
        }
        Ok(())
    }

    // ---- variable scope: global map + a stack of block-local frames ----

    fn get_var(&self, stack: &ScopeStack, name: &str) -> Option<Value> {
        for frame in stack.iter().rev() {
            if let Some(v) = frame.get(name) {
                return Some(v.clone());
            }
        }
        self.globals.get(name).cloned()
    }

    #[inline(never)]
    fn declare_var(
        &mut self,
        stack: &mut ScopeStack,
        name: &str,
        value: Value,
        line: u32,
    ) -> Result<(), WispError> {
        let target: &mut BTreeMap<String, Value> = match stack.last_mut() {
            Some(frame) => frame,
            None => &mut self.globals,
        };
        if target.contains_key(name) {
            return Err(WispError::new(
                line,
                format!("'{name}' is already declared in this scope"),
            ));
        }
        target.insert(name.to_string(), value);
        Ok(())
    }

    #[inline(never)]
    fn assign_var(
        &mut self,
        stack: &mut ScopeStack,
        name: &str,
        value: Value,
        line: u32,
    ) -> Result<(), WispError> {
        for frame in stack.iter_mut().rev() {
            if let Some(slot) = frame.get_mut(name) {
                *slot = value;
                return Ok(());
            }
        }
        if let Some(slot) = self.globals.get_mut(name) {
            *slot = value;
            return Ok(());
        }
        Err(WispError::new(
            line,
            format!("undefined variable '{name}' (did you forget 'let'?)"),
        ))
    }

    // ---- statements ----

    fn exec_block(&mut self, stmts: &[Stmt], stack: &mut ScopeStack) -> Result<Flow, WispError> {
        for stmt in stmts {
            match self.exec_stmt(stmt, stack)? {
                Flow::Normal => continue,
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    /// Run a block in its own fresh scope frame: pushed before, popped
    /// after, unconditionally (however the block ended -- normally,
    /// `break`, `continue`, or `return`). This is what makes `let` inside
    /// an `if`/`while` body local to that one pass through the block
    /// rather than leaking into the enclosing scope or colliding with
    /// itself on the next loop iteration.
    fn exec_block_scoped(
        &mut self,
        stmts: &[Stmt],
        stack: &mut ScopeStack,
    ) -> Result<Flow, WispError> {
        stack.push(BTreeMap::new());
        let result = self.exec_block(stmts, stack);
        stack.pop();
        result
    }

    /// Count one unit of native recursion against [`MAX_NESTING`].
    fn enter(&mut self, line: u32) -> Result<(), WispError> {
        if self.nesting >= MAX_NESTING {
            return Err(WispError::new(
                line,
                "stack overflow: program nests too deeply (expressions, blocks and calls combined)",
            ));
        }
        self.nesting += 1;
        Ok(())
    }

    fn exec_stmt(&mut self, stmt: &Stmt, stack: &mut ScopeStack) -> Result<Flow, WispError> {
        self.enter(stmt_line(stmt))?;
        let result = self.exec_stmt_inner(stmt, stack);
        self.nesting -= 1;
        result
    }

    // Arms outlined for the same stack-frame reason as `eval_expr_inner`.
    fn exec_stmt_inner(&mut self, stmt: &Stmt, stack: &mut ScopeStack) -> Result<Flow, WispError> {
        match stmt {
            Stmt::Let { name, expr, line } => {
                let v = self.eval_expr(expr, stack)?;
                self.declare_var(stack, name, v, *line)?;
                Ok(Flow::Normal)
            }
            Stmt::Assign { name, expr, line } => {
                let v = self.eval_expr(expr, stack)?;
                self.assign_var(stack, name, v, *line)?;
                Ok(Flow::Normal)
            }
            Stmt::ExprStmt(expr) => {
                self.eval_expr(expr, stack)?;
                Ok(Flow::Normal)
            }
            Stmt::If { branches, else_body } => self.exec_if(branches, else_body.as_deref(), stack),
            Stmt::While { cond, body, line } => self.exec_while(cond, body, *line, stack),
            Stmt::IndexAssign {
                object,
                index,
                value,
                line,
            } => self.exec_index_assign(object, index, value, *line, stack),
            Stmt::Break { line } => Ok(Flow::Break(*line)),
            Stmt::Continue { line } => Ok(Flow::Continue(*line)),
            Stmt::Return { expr } => {
                let v = match expr {
                    Some(e) => self.eval_expr(e, stack)?,
                    None => Value::Nil,
                };
                Ok(Flow::Return(v))
            }
        }
    }

    #[inline(never)]
    fn exec_if(
        &mut self,
        branches: &[(Expr, Vec<Stmt>)],
        else_body: Option<&[Stmt]>,
        stack: &mut ScopeStack,
    ) -> Result<Flow, WispError> {
        for (cond, body) in branches {
            if self.eval_bool(cond, stack)? {
                return self.exec_block_scoped(body, stack);
            }
        }
        if let Some(body) = else_body {
            return self.exec_block_scoped(body, stack);
        }
        Ok(Flow::Normal)
    }

    #[inline(never)]
    fn exec_while(&mut self, cond: &Expr, body: &[Stmt], line: u32, stack: &mut ScopeStack) -> Result<Flow, WispError> {
        loop {
            self.check_interrupts(line)?;
            if !self.eval_bool(cond, stack)? {
                break;
            }
            match self.exec_block_scoped(body, stack)? {
                Flow::Break(_) => break,
                Flow::Continue(_) | Flow::Normal => {}
                Flow::Return(v) => return Ok(Flow::Return(v)),
            }
        }
        Ok(Flow::Normal)
    }

    #[inline(never)]
    fn exec_index_assign(
        &mut self,
        object: &Expr,
        index: &Expr,
        value: &Expr,
        line: u32,
        stack: &mut ScopeStack,
    ) -> Result<Flow, WispError> {
        let obj = self.eval_expr(object, stack)?;
        let idx = self.eval_expr(index, stack)?;
        let v = self.eval_expr(value, stack)?;
        let (list, i) = index_target(&obj, &idx, line)?;
        list.borrow_mut()[i] = v;
        Ok(Flow::Normal)
    }

    // ---- expressions ----

    #[inline(never)]
    fn eval_bool(&mut self, expr: &Expr, stack: &ScopeStack) -> Result<bool, WispError> {
        let line = expr.line().unwrap_or(0);
        match self.eval_expr(expr, stack)? {
            Value::Bool(b) => Ok(b),
            other => Err(WispError::new(
                line,
                format!(
                    "expected bool here, found {} ({})",
                    other.type_name(),
                    other.to_display_string()
                ),
            )),
        }
    }

    fn eval_expr(&mut self, expr: &Expr, stack: &ScopeStack) -> Result<Value, WispError> {
        self.enter(expr.line().unwrap_or(0))?;
        let result = self.eval_expr_inner(expr, stack);
        self.nesting -= 1;
        result
    }

    // Every arm that recurses lives in its own never-inlined function.
    // The tree-walker's native stack use per nesting level is the frame
    // of `eval_expr` plus the frame of whichever arm is active; inlined,
    // every level would pay for the *union* of all arms' locals (measured
    // at roughly 2.5x the outlined cost on the UEFI target).
    fn eval_expr_inner(&mut self, expr: &Expr, stack: &ScopeStack) -> Result<Value, WispError> {
        match expr {
            Expr::IntLit(v) => Ok(Value::Int(*v)),
            Expr::StrLit(s) => Ok(Value::Str(s.clone())),
            Expr::BoolLit(b) => Ok(Value::Bool(*b)),
            Expr::NilLit => Ok(Value::Nil),
            Expr::Var { name, line } => match self.get_var(stack, name) {
                Some(v) => Ok(v),
                None => Err(undefined_variable(name, *line)),
            },
            Expr::Unary { op, expr, line } => self.eval_unary(*op, expr, *line, stack),
            Expr::Binary { op, lhs, rhs, line } => self.eval_binary(*op, lhs, rhs, *line, stack),
            Expr::Call { name, args, line } => self.eval_call(name, args, *line, stack),
            Expr::ListLit { items, line } => self.eval_list(items, *line, stack),
            Expr::Index { object, index, line } => self.eval_index(object, index, *line, stack),
        }
    }

    #[inline(never)]
    fn eval_unary(&mut self, op: UnaryOp, expr: &Expr, line: u32, stack: &ScopeStack) -> Result<Value, WispError> {
        let v = self.eval_expr(expr, stack)?;
        match (op, &v) {
            (UnaryOp::Neg, Value::Int(i)) => i
                .checked_neg()
                .map(Value::Int)
                .ok_or_else(|| WispError::new(line, "integer overflow negating i64::MIN")),
            (UnaryOp::Neg, other) => Err(WispError::new(
                line,
                format!("unary '-' expects int, found {}", other.type_name()),
            )),
            (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
            (UnaryOp::Not, other) => Err(WispError::new(
                line,
                format!("'not' expects bool, found {}", other.type_name()),
            )),
        }
    }

    #[inline(never)]
    fn eval_call(&mut self, name: &str, args: &[Expr], line: u32, stack: &ScopeStack) -> Result<Value, WispError> {
        let mut values = Vec::with_capacity(args.len());
        for a in args {
            values.push(self.eval_expr(a, stack)?);
        }
        self.call_named(name, values, line)
    }

    #[inline(never)]
    fn eval_list(&mut self, items: &[Expr], line: u32, stack: &ScopeStack) -> Result<Value, WispError> {
        if items.len() > value::MAX_LIST_LEN {
            return Err(WispError::new(
                line,
                format!("list literal is too long (over {} elements)", value::MAX_LIST_LEN),
            ));
        }
        let mut values = Vec::with_capacity(items.len());
        for it in items {
            values.push(self.eval_expr(it, stack)?);
        }
        Ok(self.lists.new_list(values))
    }

    #[inline(never)]
    fn eval_index(&mut self, object: &Expr, index: &Expr, line: u32, stack: &ScopeStack) -> Result<Value, WispError> {
        let obj = self.eval_expr(object, stack)?;
        let idx = self.eval_expr(index, stack)?;
        let (list, i) = index_target(&obj, &idx, line)?;
        let v = list.borrow()[i].clone();
        Ok(v)
    }

    #[inline(never)]
    fn eval_binary(
        &mut self,
        op: BinaryOp,
        lhs_e: &Expr,
        rhs_e: &Expr,
        line: u32,
        stack: &ScopeStack,
    ) -> Result<Value, WispError> {
        // `and`/`or` short-circuit, so they must not evaluate the RHS
        // eagerly like every other binary operator does below.
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            let op_name = if op == BinaryOp::And { "and" } else { "or" };
            let lhs = self.eval_expr(lhs_e, stack)?;
            let lb = match lhs {
                Value::Bool(b) => b,
                other => {
                    return Err(WispError::new(
                        line,
                        format!("'{op_name}' expects bool, found {}", other.type_name()),
                    ))
                }
            };
            if op == BinaryOp::And && !lb {
                return Ok(Value::Bool(false));
            }
            if op == BinaryOp::Or && lb {
                return Ok(Value::Bool(true));
            }
            let rhs = self.eval_expr(rhs_e, stack)?;
            let rb = match rhs {
                Value::Bool(b) => b,
                other => {
                    return Err(WispError::new(
                        line,
                        format!("'{op_name}' expects bool, found {}", other.type_name()),
                    ))
                }
            };
            return Ok(Value::Bool(rb));
        }

        let lhs = self.eval_expr(lhs_e, stack)?;
        let rhs = self.eval_expr(rhs_e, stack)?;

        match op {
            BinaryOp::Add => match (&lhs, &rhs) {
                (Value::Int(a), Value::Int(b)) => a
                    .checked_add(*b)
                    .map(Value::Int)
                    .ok_or_else(|| WispError::new(line, format!("integer overflow in '{a} + {b}'"))),
                (Value::Str(a), Value::Str(b)) => concat(a, b, line),
                _ => Err(WispError::new(
                    line,
                    format!(
                        "'+' expects two ints or two strings, found {} and {}",
                        lhs.type_name(),
                        rhs.type_name()
                    ),
                )),
            },
            BinaryOp::Sub => checked_int_binop(lhs, rhs, line, "-", i64::checked_sub),
            BinaryOp::Mul => checked_int_binop(lhs, rhs, line, "*", i64::checked_mul),
            BinaryOp::Div => {
                if matches!((&lhs, &rhs), (Value::Int(_), Value::Int(0))) {
                    return Err(WispError::new(line, "division by zero"));
                }
                checked_int_binop(lhs, rhs, line, "/", i64::checked_div)
            }
            BinaryOp::Rem => {
                if matches!((&lhs, &rhs), (Value::Int(_), Value::Int(0))) {
                    return Err(WispError::new(line, "modulo by zero"));
                }
                checked_int_binop(lhs, rhs, line, "%", i64::checked_rem)
            }
            BinaryOp::Eq | BinaryOp::Ne => {
                let equal = value::try_equal(&lhs, &rhs).map_err(|msg| WispError::new(line, msg))?;
                Ok(Value::Bool(equal == (op == BinaryOp::Eq)))
            }
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                compare(op, lhs, rhs, line)
            }
            BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),
        }
    }

    /// The cooperative checks run before every loop iteration and function
    /// call: the user's Esc, and the host's heap reserve.
    fn check_interrupts(&mut self, line: u32) -> Result<(), WispError> {
        if self.host.stop_requested() {
            return Err(WispError::new(line, STOPPED_MESSAGE));
        }
        if self.host.memory_low() {
            return Err(WispError::new(line, OUT_OF_MEMORY_MESSAGE));
        }
        Ok(())
    }

    #[inline(never)]
    fn call_named(&mut self, name: &str, args: Vec<Value>, line: u32) -> Result<Value, WispError> {
        if builtins::is_builtin(name) {
            let result = builtins::call(&mut *self.host, name, args, line)?;
            if name == "load" {
                // A list that arrives from the save disk becomes part of
                // this run's object graph like any other.
                self.lists.adopt(&result);
            }
            return Ok(result);
        }
        match self.functions.get(name).cloned() {
            Some(f) => self.call_function(&f, args, line),
            None => Err(WispError::new(line, format!("undefined function '{name}'"))),
        }
    }

    #[inline(never)]
    fn call_function(
        &mut self,
        func: &FuncDecl,
        args: Vec<Value>,
        line: u32,
    ) -> Result<Value, WispError> {
        self.check_interrupts(line)?;
        if args.len() != func.params.len() {
            return Err(WispError::new(
                line,
                format!(
                    "'{}' expects {} argument(s), got {}",
                    func.name,
                    func.params.len(),
                    args.len()
                ),
            ));
        }

        self.call_depth += 1;
        if self.call_depth > MAX_CALL_DEPTH {
            self.call_depth -= 1;
            return Err(WispError::new(
                line,
                "stack overflow: too much recursion (over 256 nested calls)",
            ));
        }

        // A function call gets a brand-new stack, seeded with one frame
        // holding its parameters -- it cannot see any of the caller's
        // scope frames (no closures), only true globals beyond that.
        let mut frame = BTreeMap::new();
        for (param, arg) in func.params.iter().zip(args.into_iter()) {
            frame.insert((**param).clone(), arg);
        }
        let mut stack: ScopeStack = vec![frame];

        let result = match self.enter(line) {
            Ok(()) => {
                let r = self.exec_block(&func.body, &mut stack);
                self.nesting -= 1;
                r
            }
            Err(e) => Err(e),
        };
        self.call_depth -= 1;

        match result? {
            Flow::Return(v) => Ok(v),
            Flow::Normal => Ok(Value::Nil),
            Flow::Break(line) => Err(WispError::new(line, "'break' outside of a loop")),
            Flow::Continue(line) => Err(WispError::new(line, "'continue' outside of a loop")),
        }
    }
}

/// Shared bounds/type checking for both `object[index]` (a read, in
/// `eval_expr`) and `object[index] = value;` (a write, in `exec_stmt`):
/// confirms `obj` is a list and `idx` is an in-bounds `int`, and hands
/// back the list's `Rc` handle (cheap to clone -- just a refcount bump)
/// plus a plain `usize` the caller can index with directly.
fn index_target(
    obj: &Value,
    idx: &Value,
    line: u32,
) -> Result<(List, usize), WispError> {
    let list = match obj {
        Value::List(l) => l.clone(),
        other => {
            return Err(WispError::new(
                line,
                format!("cannot index into {} (expected a list)", other.type_name()),
            ))
        }
    };
    let i = match idx {
        Value::Int(i) => *i,
        other => {
            return Err(WispError::new(
                line,
                format!("list index must be an int, found {}", other.type_name()),
            ))
        }
    };
    let len = list.borrow().len();
    if i < 0 || i as usize >= len {
        return Err(WispError::new(
            line,
            format!("list index {i} out of bounds (length {len})"),
        ));
    }
    Ok((list, i as usize))
}

fn checked_int_binop(
    lhs: Value,
    rhs: Value,
    line: u32,
    symbol: &str,
    f: impl Fn(i64, i64) -> Option<i64>,
) -> Result<Value, WispError> {
    match (lhs, rhs) {
        (Value::Int(a), Value::Int(b)) => f(a, b)
            .map(Value::Int)
            .ok_or_else(|| WispError::new(line, format!("integer overflow in '{a} {symbol} {b}'"))),
        (a, b) => Err(WispError::new(
            line,
            format!(
                "'{symbol}' expects two ints, found {} and {}",
                a.type_name(),
                b.type_name()
            ),
        )),
    }
}

fn compare(op: BinaryOp, lhs: Value, rhs: Value, line: u32) -> Result<Value, WispError> {
    let ordering = match (&lhs, &rhs) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Str(a), Value::Str(b)) => a.as_str().cmp(b.as_str()),
        (a, b) => {
            return Err(WispError::new(
                line,
                format!(
                    "comparison expects two ints or two strings, found {} and {}",
                    a.type_name(),
                    b.type_name()
                ),
            ))
        }
    };
    let result = match op {
        BinaryOp::Lt => ordering.is_lt(),
        BinaryOp::Le => ordering.is_le(),
        BinaryOp::Gt => ordering.is_gt(),
        BinaryOp::Ge => ordering.is_ge(),
        _ => unreachable!("compare() only called for Lt/Le/Gt/Ge"),
    };
    Ok(Value::Bool(result))
}

/// The source line a statement reports errors at, where it has one.
fn stmt_line(stmt: &Stmt) -> u32 {
    match stmt {
        Stmt::Let { line, .. }
        | Stmt::Assign { line, .. }
        | Stmt::IndexAssign { line, .. }
        | Stmt::While { line, .. }
        | Stmt::Break { line }
        | Stmt::Continue { line } => *line,
        Stmt::ExprStmt(e) | Stmt::Return { expr: Some(e) } => e.line().unwrap_or(0),
        Stmt::If { branches, .. } => branches.first().and_then(|(c, _)| c.line()).unwrap_or(0),
        Stmt::Return { expr: None } => 0,
    }
}

/// String `+`, refusing results over [`value::MAX_STR_LEN`] bytes and
/// reporting allocation failure as a script error rather than a kernel
/// halt.
fn concat(a: &str, b: &str, line: u32) -> Result<Value, WispError> {
    let len = a.len() + b.len();
    if len > value::MAX_STR_LEN {
        return Err(WispError::new(
            line,
            format!("string is too long ({len} bytes; the limit is {})", value::MAX_STR_LEN),
        ));
    }
    let mut out = String::new();
    out.try_reserve_exact(len)
        .map_err(|_| WispError::new(line, OUT_OF_MEMORY_MESSAGE))?;
    out.push_str(a);
    out.push_str(b);
    Ok(Value::Str(Rc::new(out)))
}

#[cold]
#[inline(never)]
fn undefined_variable(name: &str, line: u32) -> WispError {
    WispError::new(line, format!("undefined variable '{name}' (did you forget 'let'?)"))
}
