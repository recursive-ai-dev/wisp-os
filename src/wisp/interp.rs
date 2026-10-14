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
use core::cell::RefCell;

use super::ast::{BinaryOp, Expr, FuncDecl, Program, Stmt, TopItem, UnaryOp};
use super::builtins;
use super::value::Value;
use super::WispError;

/// Recursion guard: a user function calling itself (directly or via a
/// cycle) more than this many times deep is reported as a clean runtime
/// error instead of exhausting this interpreter's real call stack (which,
/// since this is a tree-walker, backs every WISP call with a real Rust
/// stack frame) and taking the whole firmware shell down with it.
const MAX_CALL_DEPTH: usize = 256;

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
    /// Overwrites any value already saved under `key`. `Err` covers both
    /// "no save disk available this boot" and every `save_format::save`
    /// failure (key too long, value too large, all slots full).
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
}

impl<'h> Interp<'h> {
    pub fn new(host: &'h mut dyn Host) -> Self {
        Self {
            host,
            globals: BTreeMap::new(),
            functions: BTreeMap::new(),
            call_depth: 0,
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
                self.functions
                    .insert((*f.name).clone(), Rc::new(f.clone()));
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

    fn exec_stmt(&mut self, stmt: &Stmt, stack: &mut ScopeStack) -> Result<Flow, WispError> {
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
            Stmt::If { branches, else_body } => {
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
            Stmt::While { cond, body, line } => {
                loop {
                    if self.host.stop_requested() {
                        return Err(WispError::new(*line, STOPPED_MESSAGE));
                    }
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
            Stmt::IndexAssign {
                object,
                index,
                value,
                line,
            } => {
                let obj = self.eval_expr(object, stack)?;
                let idx = self.eval_expr(index, stack)?;
                let v = self.eval_expr(value, stack)?;
                let (list, i) = index_target(&obj, &idx, *line)?;
                list.borrow_mut()[i] = v;
                Ok(Flow::Normal)
            }
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

    // ---- expressions ----

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
        match expr {
            Expr::IntLit(v) => Ok(Value::Int(*v)),
            Expr::StrLit(s) => Ok(Value::Str(s.clone())),
            Expr::BoolLit(b) => Ok(Value::Bool(*b)),
            Expr::NilLit => Ok(Value::Nil),
            Expr::Var { name, line } => self.get_var(stack, name).ok_or_else(|| {
                WispError::new(
                    *line,
                    format!("undefined variable '{name}' (did you forget 'let'?)"),
                )
            }),
            Expr::Unary { op, expr, line } => {
                let v = self.eval_expr(expr, stack)?;
                match (op, &v) {
                    (UnaryOp::Neg, Value::Int(i)) => i
                        .checked_neg()
                        .map(Value::Int)
                        .ok_or_else(|| WispError::new(*line, "integer overflow negating i64::MIN")),
                    (UnaryOp::Neg, other) => Err(WispError::new(
                        *line,
                        format!("unary '-' expects int, found {}", other.type_name()),
                    )),
                    (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
                    (UnaryOp::Not, other) => Err(WispError::new(
                        *line,
                        format!("'not' expects bool, found {}", other.type_name()),
                    )),
                }
            }
            Expr::Binary { op, lhs, rhs, line } => self.eval_binary(*op, lhs, rhs, *line, stack),
            Expr::Call { name, args, line } => {
                let mut values = Vec::with_capacity(args.len());
                for a in args {
                    values.push(self.eval_expr(a, stack)?);
                }
                self.call_named(name, values, *line)
            }
            Expr::ListLit { items, .. } => {
                let mut values = Vec::with_capacity(items.len());
                for it in items {
                    values.push(self.eval_expr(it, stack)?);
                }
                Ok(Value::List(Rc::new(RefCell::new(values))))
            }
            Expr::Index { object, index, line } => {
                let obj = self.eval_expr(object, stack)?;
                let idx = self.eval_expr(index, stack)?;
                let (list, i) = index_target(&obj, &idx, *line)?;
                let v = list.borrow()[i].clone();
                Ok(v)
            }
        }
    }

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
                (Value::Str(a), Value::Str(b)) => Ok(Value::Str(Rc::new(format!("{a}{b}")))),
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
            BinaryOp::Eq => Ok(Value::Bool(lhs == rhs)),
            BinaryOp::Ne => Ok(Value::Bool(lhs != rhs)),
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                compare(op, lhs, rhs, line)
            }
            BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),
        }
    }

    fn call_named(&mut self, name: &str, args: Vec<Value>, line: u32) -> Result<Value, WispError> {
        if builtins::is_builtin(name) {
            return builtins::call(&mut *self.host, name, args, line);
        }
        match self.functions.get(name).cloned() {
            Some(f) => self.call_function(&f, args, line),
            None => Err(WispError::new(line, format!("undefined function '{name}'"))),
        }
    }

    fn call_function(
        &mut self,
        func: &FuncDecl,
        args: Vec<Value>,
        line: u32,
    ) -> Result<Value, WispError> {
        if self.host.stop_requested() {
            return Err(WispError::new(line, STOPPED_MESSAGE));
        }
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

        let result = self.exec_block(&func.body, &mut stack);
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
) -> Result<(Rc<RefCell<Vec<Value>>>, usize), WispError> {
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
