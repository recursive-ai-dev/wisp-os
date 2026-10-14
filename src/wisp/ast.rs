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
    /// `[a, b, c]` -- a list literal. Each element expression is
    /// evaluated in order to build a fresh list every time this
    /// expression runs (so `[1, 2]` inside a loop body makes a new list
    /// each iteration, not the same one reused).
    ListLit {
        items: Vec<Expr>,
        line: u32,
    },
    /// `object[index]` -- a list read. `object` must evaluate to a list
    /// and `index` to an in-bounds `int`; both are checked at runtime.
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
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
            | Expr::ListLit { line, .. }
            | Expr::Index { line, .. } => Some(*line),
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
    /// `object[index] = value;` -- a list write. The only other way to
    /// mutate a list's contents besides `push`/`pop`. All three of
    /// `object`, `index`, and `value` are ordinary expressions,
    /// (re-)evaluated fresh every time this statement runs.
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
    Func(FuncDecl),
    Stmt(Stmt),
}

#[derive(Debug, Clone)]
pub struct Program {
    pub items: Vec<TopItem>,
}
