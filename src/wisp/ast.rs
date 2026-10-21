//! Abstract syntax tree for WISP, per `docs/WISP_LANGUAGE.md`.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone)]
pub enum Expr {
    IntLit(i64),
    StrLit(Rc<String>),
    BoolLit(bool),
    NilLit,
    Var {
        name: Rc<String>,
        line: u32,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
        line: u32,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        line: u32,
    },
    Call {
        name: Rc<String>,
        args: Vec<Expr>,
        line: u32,
    },
    /// `callee(args)` where `callee` is any expression other than a bare
    /// name: `table[word](rest)`, `make()(3)`, `(f)(3)`. It must evaluate
    /// to a function value. A bare `name(args)` is [`Expr::Call`].
    CallValue {
        callee: Box<Expr>,
        args: Vec<Expr>,
        line: u32,
    },
    /// `[a, b, c]` -- a list literal. Each element expression is
    /// evaluated in order to build a fresh list every time this
    /// expression runs (so `[1, 2]` inside a loop body makes a new list
    /// each iteration, not the same one reused).
    ListLit {
        items: Vec<Expr>,
        line: u32,
    },
    /// `{key: value, ...}` -- a map literal. Like a list literal, it
    /// builds a fresh map every time it runs, evaluating each key and then
    /// its value, entry by entry in source order.
    MapLit {
        entries: Vec<(Expr, Expr)>,
        line: u32,
    },
    /// `object[index]` -- a list element, the value a map holds under the
    /// key `index`, or the character starting at byte offset `index` of a
    /// string. `object` must evaluate to a list, a map or a string, and
    /// `index` to an in-bounds `int` (a key present in the map); both are
    /// checked at runtime.
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
        line: u32,
    },
    /// `object[start:end]`, either bound optional (`a[2:]`, `a[:3]`,
    /// `a[:]`) -- a new list holding a copy of that run of elements, or the
    /// substring between those byte offsets of a string.
    Slice {
        object: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        line: u32,
    },
}

impl Expr {
    /// The source line this expression starts on, used for runtime error
    /// reporting. Literals don't carry a line of their own (the parser
    /// never needs to report an error rooted at a bare literal), so this
    /// is only meaningful for the variants that do.
    pub fn line(&self) -> Option<u32> {
        match self {
            Expr::Var { line, .. }
            | Expr::Unary { line, .. }
            | Expr::Binary { line, .. }
            | Expr::Call { line, .. }
            | Expr::CallValue { line, .. }
            | Expr::ListLit { line, .. }
            | Expr::MapLit { line, .. }
            | Expr::Index { line, .. }
            | Expr::Slice { line, .. } => Some(*line),
            Expr::IntLit(_) | Expr::StrLit(_) | Expr::BoolLit(_) | Expr::NilLit => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name: Rc<String>,
        expr: Expr,
        line: u32,
    },
    Assign {
        name: Rc<String>,
        expr: Expr,
        line: u32,
    },
    ExprStmt(Expr),
    /// `object[index] = value;` -- a list element write, or a map entry
    /// write (adding the key if it is new). All three of `object`,
    /// `index`, and `value` are ordinary expressions, (re-)evaluated fresh
    /// every time this statement runs.
    IndexAssign {
        object: Expr,
        index: Expr,
        value: Expr,
        line: u32,
    },
    /// `if`/`elif`*/`else`, modeled as an ordered list of
    /// (condition, body) branches tried in turn, plus an optional final
    /// `else` body. The parser is responsible for folding `elif` chains
    /// into this flat list rather than nesting them.
    If {
        branches: Vec<(Expr, Vec<Stmt>)>,
        else_body: Option<Vec<Stmt>>,
    },
    /// `line` is the `while` keyword's own line. A bad condition is still
    /// reported at the condition expression's line (`Expr::line()`, more
    /// precise); `line` is what a stopped program (Esc) reports, since a
    /// condition like `true` carries no line of its own.
    While {
        cond: Expr,
        body: Vec<Stmt>,
        line: u32,
    },
    /// `for var in iter { body }`. `iter` is evaluated once, before the
    /// first pass; a list is looped over as it was at that moment (a
    /// snapshot of its elements), a string one character at a time. Each
    /// pass runs `body` in a fresh scope holding `var`. A map is looped
    /// over by its keys, as they were at that moment. `line` is the
    /// `for` keyword's, for the same reason as `While`'s.
    For {
        var: Rc<String>,
        iter: Expr,
        body: Vec<Stmt>,
        line: u32,
    },
    Break {
        line: u32,
    },
    Continue {
        line: u32,
    },
    /// No `line` here either: WISP places no type constraint on what a
    /// function returns, so there is no error `return` itself could ever
    /// raise that needs to cite its own position.
    Return {
        expr: Option<Expr>,
    },
}

#[derive(Debug, Clone)]
pub struct FuncDecl {
    pub name: Rc<String>,
    pub params: Vec<Rc<String>>,
    pub body: Vec<Stmt>,
    pub line: u32,
}

/// A top-level item is either a function declaration or an ordinary
/// statement; per the language spec, all `Func` items are registered
/// before any `Stmt` item runs, regardless of their order in the file, so
/// a top-level statement can call a function declared later in the
/// source text.
#[derive(Debug, Clone)]
pub enum TopItem {
    /// Shared, so registering a function with the interpreter is a
    /// refcount bump rather than a deep copy of its whole body.
    Func(Rc<FuncDecl>),
    Stmt(Stmt),
}

#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<TopItem>,
}
