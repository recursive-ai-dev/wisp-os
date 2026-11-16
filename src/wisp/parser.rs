//! Recursive-descent parser: `Vec<Token>` -> `ast::Program`. Precedence
//! climbing follows the table in `docs/WISP_LANGUAGE.md` exactly, lowest
//! to highest: `or`, `and`, `==`/`!=`, comparisons, `+`/`-`, `*`/`/`/`%`,
//! unary `not`/`-`, then calls/grouping/literals (including `[...]` list
//! and `{...}` map literals).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::ast::{BinaryOp, Expr, FuncDecl, Lambda, Program, Stmt, TopItem, UnaryOp};
use super::lexer::{Tok, Token};
use super::WispError;

/// Deepest syntax tree the parser builds, counting every nested block,
/// parenthesized/sub-expression, unary operator, index, and each link of
/// a binary-operator chain (`1 + 1 + 1` is a left-leaning tree as deep as
/// it is long). Everything that later walks the tree -- evaluation,
/// dropping it -- recurses once per level on the kernel stack, so the
/// limit is enforced while parsing, before an oversized tree exists.
/// Real programs stay in the low tens.
pub const MAX_SYNTAX_DEPTH: usize = 128;

pub fn parse(tokens: &[Token]) -> Result<Program, WispError> {
    let mut p = Parser { tokens, pos: 0, depth: 0, height: 0 };
    let mut items = Vec::new();
    while !p.is_eof() {
        // `fn name` declares; `fn (` starts an expression statement.
        if p.check(&Tok::Fn) && !matches!(p.peek_at(1), Tok::LParen) {
            items.push(TopItem::Func(alloc::rc::Rc::new(p.parse_func_decl()?)));
        } else {
            items.push(TopItem::Stmt(p.parse_stmt()?));
        }
    }
    Ok(Program { items })
}

struct Parser<'t> {
    tokens: &'t [Token],
    pos: usize,
    /// Syntax-tree depth of the node currently being parsed; see
    /// [`MAX_SYNTAX_DEPTH`].
    depth: usize,
    /// Height of the most recently returned expression, statement or block.
    /// Unlike `depth` (active parser recursion), this survives a grouped
    /// expression followed by more operators. Every parent combines its
    /// children's heights before returning, so building and dropping the
    /// resulting AST has the same bound even across grammar productions.
    height: usize,
}

impl<'t> Parser<'t> {
    fn finish<T>(&mut self, node: T, height: usize) -> Result<T, WispError> {
        if height > MAX_SYNTAX_DEPTH {
            return Err(WispError::new(
                self.peek_line(),
                format!("program is nested too deeply (over {MAX_SYNTAX_DEPTH} levels of blocks/expressions)"),
            ));
        }
        self.height = height;
        Ok(node)
    }

    /// Go one level deeper into the syntax tree, failing cleanly past
    /// [`MAX_SYNTAX_DEPTH`]. Callers restore `self.depth` on the way out;
    /// on an error the whole parse is abandoned, so no restore is needed.
    fn descend(&mut self) -> Result<(), WispError> {
        self.depth += 1;
        if self.depth > MAX_SYNTAX_DEPTH {
            return Err(WispError::new(
                self.peek_line(),
                format!("program is nested too deeply (over {MAX_SYNTAX_DEPTH} levels of blocks/expressions)"),
            ));
        }
        Ok(())
    }

    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn peek_line(&self) -> u32 {
        self.tokens[self.pos].line
    }

    fn peek_col(&self) -> u32 {
        self.tokens[self.pos].col
    }

    fn peek_at(&self, offset: usize) -> Tok {
        self.tokens
            .get(self.pos + offset)
            .map(|t| t.tok.clone())
            .unwrap_or(Tok::Eof)
    }

    fn is_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn check(&self, want: &Tok) -> bool {
        core::mem::discriminant(self.peek()) == core::mem::discriminant(want)
    }

    fn match_tok(&mut self, want: &Tok) -> bool {
        if self.check(want) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, want: Tok) -> Result<Token, WispError> {
        let line = self.peek_line();
        let col = self.peek_col();
        if self.check(&want) {
            Ok(self.advance())
        } else {
            Err(WispError::new(
                line,
                format!("expected {}, found {}", describe(&want), describe(self.peek())),
            )
            .with_col(col))
        }
    }

    fn expect_ident(&mut self) -> Result<alloc::rc::Rc<String>, WispError> {
        let line = self.peek_line();
        let col = self.peek_col();
        match self.advance().tok {
            Tok::Ident(name) => Ok(name),
            other => Err(WispError::new(
                line,
                format!("expected identifier, found {}", describe(&other)),
            )
            .with_col(col)),
        }
    }

    // ---- top-level items ----

    #[inline(never)]
    fn parse_func_decl(&mut self) -> Result<FuncDecl, WispError> {
        let line = self.peek_line();
        self.expect(Tok::Fn)?;
        let name = self.expect_ident()?;
        let (params, body) = self.parse_params_and_body()?;
        self.finish(FuncDecl {
            name,
            params,
            body,
            line,
        }, self.height + 1)
    }

    /// `(a, b) { ... }`: what a named declaration and an anonymous function
    /// share after the `fn` (and the name, for the former).
    fn parse_params_and_body(&mut self) -> Result<(Vec<alloc::rc::Rc<String>>, Vec<Stmt>), WispError> {
        self.expect(Tok::LParen)?;
        let mut params = Vec::new();
        if !self.check(&Tok::RParen) {
            loop {
                params.push(self.expect_ident()?);
                if self.match_tok(&Tok::Comma) {
                    continue;
                }
                break;
            }
        }
        self.expect(Tok::RParen)?;
        let body = self.parse_block()?;
        Ok((params, body))
    }

    /// An anonymous function, after its `fn`. Outlined from
    /// `parse_primary` so that its locals are not paid at every level of
    /// expression nesting.
    #[inline(never)]
    fn parse_lambda(&mut self, line: u32) -> Result<alloc::rc::Rc<Lambda>, WispError> {
        if let Tok::Ident(name) = self.peek() {
            return Err(WispError::new(
                self.peek_line(),
                format!(
                    "'fn {name}' is a declaration and can only appear at the top level of a program \
                     (for a function value here, write 'fn (parameters) {{ ... }}' with no name)"
                ),
            ));
        }
        let (params, body) = self.parse_params_and_body()?;
        self.finish(alloc::rc::Rc::new(Lambda { name: None, params, body, line }), self.height + 1)
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, WispError> {
        self.descend()?;
        self.expect(Tok::LBrace)?;
        let mut stmts = Vec::new();
        let mut height = 0;
        while !self.check(&Tok::RBrace) {
            if self.is_eof() {
                return Err(WispError::new(
                    self.peek_line(),
                    "unterminated block, expected '}'",
                ));
            }
            stmts.push(self.parse_stmt()?);
            height = height.max(self.height);
        }
        self.expect(Tok::RBrace)?;
        self.depth -= 1;
        self.finish(stmts, height + 1)
    }

    // ---- statements ----

    fn parse_stmt(&mut self) -> Result<Stmt, WispError> {
        match self.peek() {
            Tok::Let => self.parse_let(),
            Tok::If => self.parse_if(),
            Tok::While => self.parse_while(),
            Tok::For => self.parse_for(),
            Tok::Break => {
                let line = self.peek_line();
                self.advance();
                self.expect(Tok::Semi)?;
                self.finish(Stmt::Break { line }, 1)
            }
            Tok::Continue => {
                let line = self.peek_line();
                self.advance();
                self.expect(Tok::Semi)?;
                self.finish(Stmt::Continue { line }, 1)
            }
            Tok::Return => self.parse_return(),
            Tok::Ident(_) if assign_op(&self.peek_at(1)).is_some() => self.parse_assign(),
            _ => self.parse_index_assign_or_expr_stmt(),
        }
    }

    #[inline(never)]
    fn parse_let(&mut self) -> Result<Stmt, WispError> {
        let line = self.peek_line();
        self.expect(Tok::Let)?;
        let name = self.expect_ident()?;
        self.expect(Tok::Eq)?;
        let expr = self.parse_expr()?;
        self.expect(Tok::Semi)?;
        self.finish(Stmt::Let { name, expr, line }, self.height + 1)
    }

    #[inline(never)]
    fn parse_assign(&mut self) -> Result<Stmt, WispError> {
        let line = self.peek_line();
        let name = self.expect_ident()?;
        let op = assign_op(self.peek()).expect("checked by parse_stmt");
        self.advance();
        let expr = self.parse_expr()?;
        self.expect(Tok::Semi)?;
        self.finish(Stmt::Assign { name, op, expr, line }, self.height + 1)
    }

    /// Handles a statement that starts `ident [` -- either a list write
    /// (`name[idx] = value;`, possibly through several chained indices
    /// like `grid[y][x] = value;`) or, if no `=` follows the index chain,
    /// an ordinary expression statement that happens to start with a list
    /// read (e.g. a bare `queue[0];`, which is legal but pointless).
    /// Both start identically -- an identifier followed by one or more
    /// `[...]` -- and only diverge at the token right after that chain,
    /// so this parses the postfix target once and branches on what comes
    /// next. Parse the whole expression so an indexed read can also be
    /// followed by binary operators, without backtracking.
    #[inline(never)]
    fn parse_index_assign_or_expr_stmt(&mut self) -> Result<Stmt, WispError> {
        let line = self.peek_line();
        let target = self.parse_expr()?;
        let target_height = self.height;
        if let Some(op) = assign_op(self.peek()) {
            self.advance();
            if !matches!(target, Expr::Index { .. } | Expr::Slice { .. }) {
                return Err(WispError::new(line, "invalid assignment target (expected an indexed element)"));
            }
            let value = self.parse_expr()?;
            self.expect(Tok::Semi)?;
            return match target {
                Expr::Index { object, index, .. } => self.finish(Stmt::IndexAssign {
                    object: *object,
                    index: *index,
                    op,
                    value,
                    line,
                }, target_height.max(self.height) + 1),
                Expr::Slice { .. } => Err(WispError::new(
                    line,
                    "cannot assign to a slice (assign one element at a time, or use insert()/remove())",
                )),
                _ => Err(WispError::new(line, "invalid assignment target (expected an indexed element)")),
            };
        }
        self.expect(Tok::Semi)?;
        self.finish(Stmt::ExprStmt(target), target_height + 1)
    }

    #[inline(never)]
    fn parse_if(&mut self) -> Result<Stmt, WispError> {
        self.expect(Tok::If)?;
        let mut branches = Vec::new();
        let cond = self.parse_expr()?;
        let mut height = self.height;
        let body = self.parse_block()?;
        height = height.max(self.height);
        branches.push((cond, body));

        let else_body = loop {
            if self.match_tok(&Tok::Elif) {
                let cond = self.parse_expr()?;
                height = height.max(self.height);
                let body = self.parse_block()?;
                height = height.max(self.height);
                branches.push((cond, body));
                continue;
            }
            if self.match_tok(&Tok::Else) {
                let body = self.parse_block()?;
                height = height.max(self.height);
                break Some(body);
            }
            break None;
        };

        self.finish(Stmt::If { branches, else_body }, height + 1)
    }

    #[inline(never)]
    fn parse_while(&mut self) -> Result<Stmt, WispError> {
        let line = self.expect(Tok::While)?.line;
        let cond = self.parse_expr()?;
        let condition_height = self.height;
        let body = self.parse_block()?;
        self.finish(Stmt::While { cond, body, line }, condition_height.max(self.height) + 1)
    }

    #[inline(never)]
    fn parse_for(&mut self) -> Result<Stmt, WispError> {
        let line = self.expect(Tok::For)?.line;
        let var = self.expect_ident()?;
        self.expect(Tok::In)?;
        let iter = self.parse_expr()?;
        let iter_height = self.height;
        let body = self.parse_block()?;
        self.finish(Stmt::For { var, iter, body, line }, iter_height.max(self.height) + 1)
    }

    #[inline(never)]
    fn parse_return(&mut self) -> Result<Stmt, WispError> {
        self.expect(Tok::Return)?;
        self.height = 0;
        let expr = if self.check(&Tok::Semi) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(Tok::Semi)?;
        self.finish(Stmt::Return { expr }, self.height + 1)
    }

    // ---- expressions, lowest to highest precedence ----

    fn parse_expr(&mut self) -> Result<Expr, WispError> {
        self.descend()?;
        let expr = self.parse_binary(1)?;
        self.depth -= 1;
        Ok(expr)
    }

    /// Every binary operator at or above `min_prec`, by precedence
    /// climbing (lowest to highest: `or`, `and`, `==`/`!=`, comparisons,
    /// `+`/`-`, `*`/`/`/`%`; all left-associative). One function for all
    /// six levels rather than one per level: a parenthesized
    /// sub-expression then costs one native stack frame here instead of
    /// six, which is most of what bounds how deep `MAX_SYNTAX_DEPTH` can
    /// safely be.
    fn parse_binary(&mut self, min_prec: u8) -> Result<Expr, WispError> {
        let base = self.depth;
        let mut lhs = self.parse_unary()?;
        let mut height = self.height;
        while let Some((op, prec)) = binary_op(self.peek()) {
            if prec < min_prec {
                break;
            }
            let line = self.peek_line();
            self.advance();
            self.descend()?;
            let rhs = self.parse_binary(prec + 1)?;
            height = height.max(self.height) + 1;
            lhs = self.finish(Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            }, height)?;
        }
        self.depth = base;
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, WispError> {
        let line = self.peek_line();
        if self.match_tok(&Tok::Not) {
            self.descend()?;
            let expr = self.parse_unary()?;
            self.depth -= 1;
            return self.finish(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
                line,
            }, self.height + 1);
        }
        if self.match_tok(&Tok::Minus) {
            self.descend()?;
            let expr = self.parse_unary()?;
            self.depth -= 1;
            return self.finish(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
                line,
            }, self.height + 1);
        }
        self.parse_postfix()
    }

    /// Wraps a primary expression in zero or more `[index]` reads or
    /// `[start:end]` slices, e.g. `grid[y][x]`, `make_list()[0]` or
    /// `name[1:]`. Binds tighter than unary `not`/`-` and looser than
    /// nothing (it's the tightest-binding construct in the language, same
    /// as function calls).
    fn parse_postfix(&mut self) -> Result<Expr, WispError> {
        let base = self.depth;
        let mut expr = self.parse_primary()?;
        while self.check(&Tok::LBracket) || self.check(&Tok::LParen) {
            let line = self.peek_line();
            if self.match_tok(&Tok::LParen) {
                expr = self.parse_call_suffix(expr, line)?;
                continue;
            }
            expr = self.parse_index_suffix(expr, line)?;
        }
        self.depth = base;
        Ok(expr)
    }

    /// An index or a slice of `object`, from its `[` through its `]`.
    /// Outlined so that its locals are not paid at every level of
    /// expression nesting.
    #[inline(never)]
    fn parse_index_suffix(&mut self, object: Expr, line: u32) -> Result<Expr, WispError> {
        let mut height = self.height;
        self.advance();
        self.descend()?;
        let first = if self.check(&Tok::Colon) { None } else { Some(Box::new(self.parse_expr()?)) };
        if first.is_some() {
            height = height.max(self.height);
        }
        if self.match_tok(&Tok::Colon) {
            let end = if self.check(&Tok::RBracket) { None } else { Some(Box::new(self.parse_expr()?)) };
            if end.is_some() {
                height = height.max(self.height);
            }
            self.expect(Tok::RBracket)?;
            self.finish(Expr::Slice { object: Box::new(object), start: first, end, line }, height + 1)
        } else {
            self.expect(Tok::RBracket)?;
            self.finish(Expr::Index {
                object: Box::new(object),
                index: first.expect("an index expression was parsed: the token after '[' was not ':'"),
                line,
            }, height + 1)
        }
    }

    /// A call on something that is not a bare name -- the result of an
    /// index, a call or a parenthesized expression -- after its `(`.
    /// (`name(` was taken by `parse_primary`.) Outlined so its locals are
    /// not paid at every level of expression nesting.
    #[inline(never)]
    fn parse_call_suffix(&mut self, callee: Expr, line: u32) -> Result<Expr, WispError> {
        let callee_height = self.height;
        self.descend()?;
        let args = self.parse_call_args()?;
        self.finish(Expr::CallValue { callee: Box::new(callee), args, line }, callee_height.max(self.height) + 1)
    }

    /// A call's arguments, after its `(`, through its `)`. Outlined from
    /// `parse_primary` so its locals are not paid at every level of
    /// expression nesting.
    #[inline(never)]
    fn parse_call_args(&mut self) -> Result<Vec<Expr>, WispError> {
        let mut args = Vec::new();
        let mut height = 0;
        if !self.check(&Tok::RParen) {
            loop {
                args.push(self.parse_expr()?);
                height = height.max(self.height);
                if self.match_tok(&Tok::Comma) {
                    continue;
                }
                break;
            }
        }
        self.expect(Tok::RParen)?;
        self.finish(args, height)
    }

    fn parse_primary(&mut self) -> Result<Expr, WispError> {
        let line = self.peek_line();
        let col = self.peek_col();
        match self.advance().tok {
            Tok::Fn => Ok(Expr::Lambda(self.parse_lambda(line)?)),
            Tok::Int(v) => self.finish(Expr::IntLit(v, line), 0),
            Tok::Str(s) => self.finish(Expr::StrLit(s, line), 0),
            Tok::True => self.finish(Expr::BoolLit(true, line), 0),
            Tok::False => self.finish(Expr::BoolLit(false, line), 0),
            Tok::Nil => self.finish(Expr::NilLit(line), 0),
            Tok::Ident(name) => {
                if self.match_tok(&Tok::LParen) {
                    let args = self.parse_call_args()?;
                    self.finish(Expr::Call { name, args, line }, self.height + 1)
                } else {
                    self.finish(Expr::Var { name, line }, 0)
                }
            }
            Tok::LParen => {
                let expr = self.parse_expr()?;
                self.expect(Tok::RParen)?;
                Ok(expr)
            }
            Tok::LBracket => {
                let mut items = Vec::new();
                let mut height = 0;
                if !self.check(&Tok::RBracket) {
                    loop {
                        items.push(self.parse_expr()?);
                        height = height.max(self.height);
                        if self.match_tok(&Tok::Comma) {
                            continue;
                        }
                        break;
                    }
                }
                self.expect(Tok::RBracket)?;
                self.finish(Expr::ListLit { items, line }, height + 1)
            }
            // Only reached where an operand is expected, so a `{` here is
            // never a block: a block's `{` comes after a complete condition
            // or a `)`, where the expression parser has already stopped.
            Tok::LBrace => {
                let entries = self.parse_map_entries()?;
                self.finish(Expr::MapLit { entries, line }, self.height + 1)
            }
            other => Err(WispError::new(line, format!("unexpected {}", describe(&other))).with_col(col)),
        }
    }

    /// The entries of a map literal, after its `{`, through its `}`.
    /// Outlined from `parse_primary` so that its locals are not paid at
    /// every level of expression nesting.
    #[inline(never)]
    fn parse_map_entries(&mut self) -> Result<Vec<(Expr, Expr)>, WispError> {
        let mut entries = Vec::new();
        let mut height = 0;
        if !self.check(&Tok::RBrace) {
            loop {
                let key = self.parse_expr()?;
                height = height.max(self.height);
                if !self.match_tok(&Tok::Colon) {
                    return Err(WispError::new(
                        self.peek_line(),
                        format!("expected ':' after a map key, found {}", describe(self.peek())),
                    ));
                }
                let value = self.parse_expr()?;
                height = height.max(self.height);
                entries.push((key, value));
                if self.match_tok(&Tok::Comma) {
                    continue;
                }
                break;
            }
        }
        self.expect(Tok::RBrace)?;
        self.finish(entries, height)
    }
}

/// Assignment token (`=` or compound `+=`, `-=`, `*=`, `/=`, `%=`).
fn assign_op(tok: &Tok) -> Option<Option<BinaryOp>> {
    Some(match tok {
        Tok::Eq => None,
        Tok::PlusEq => Some(BinaryOp::Add),
        Tok::MinusEq => Some(BinaryOp::Sub),
        Tok::StarEq => Some(BinaryOp::Mul),
        Tok::SlashEq => Some(BinaryOp::Div),
        Tok::PercentEq => Some(BinaryOp::Rem),
        _ => return None,
    })
}

/// A binary operator token's operator and precedence (higher binds
/// tighter), or `None` if `tok` is not one.
fn binary_op(tok: &Tok) -> Option<(BinaryOp, u8)> {
    Some(match tok {
        Tok::Or => (BinaryOp::Or, 1),
        Tok::And => (BinaryOp::And, 2),
        Tok::EqEq => (BinaryOp::Eq, 3),
        Tok::NotEq => (BinaryOp::Ne, 3),
        Tok::Lt => (BinaryOp::Lt, 4),
        Tok::Le => (BinaryOp::Le, 4),
        Tok::Gt => (BinaryOp::Gt, 4),
        Tok::Ge => (BinaryOp::Ge, 4),
        Tok::Plus => (BinaryOp::Add, 5),
        Tok::Minus => (BinaryOp::Sub, 5),
        Tok::Star => (BinaryOp::Mul, 6),
        Tok::Slash => (BinaryOp::Div, 6),
        Tok::Percent => (BinaryOp::Rem, 6),
        _ => return None,
    })
}

fn describe(tok: &Tok) -> String {
    match tok {
        Tok::Int(v) => format!("integer '{v}'"),
        Tok::Str(_) => "string literal".into(),
        Tok::Ident(name) => format!("identifier '{name}'"),
        Tok::True => "'true'".into(),
        Tok::False => "'false'".into(),
        Tok::Nil => "'nil'".into(),
        Tok::Let => "'let'".into(),
        Tok::If => "'if'".into(),
        Tok::Elif => "'elif'".into(),
        Tok::Else => "'else'".into(),
        Tok::While => "'while'".into(),
        Tok::For => "'for'".into(),
        Tok::In => "'in'".into(),
        Tok::Fn => "'fn'".into(),
        Tok::Return => "'return'".into(),
        Tok::Break => "'break'".into(),
        Tok::Continue => "'continue'".into(),
        Tok::And => "'and'".into(),
        Tok::Or => "'or'".into(),
        Tok::Not => "'not'".into(),
        Tok::Plus => "'+'".into(),
        Tok::Minus => "'-'".into(),
        Tok::Star => "'*'".into(),
        Tok::Slash => "'/'".into(),
        Tok::Percent => "'%'".into(),
        Tok::PlusEq => "'+='".into(),
        Tok::MinusEq => "'-='".into(),
        Tok::StarEq => "'*='".into(),
        Tok::SlashEq => "'/='".into(),
        Tok::PercentEq => "'%='".into(),
        Tok::EqEq => "'=='".into(),
        Tok::NotEq => "'!='".into(),
        Tok::Lt => "'<'".into(),
        Tok::Le => "'<='".into(),
        Tok::Gt => "'>'".into(),
        Tok::Ge => "'>='".into(),
        Tok::Eq => "'='".into(),
        Tok::LParen => "'('".into(),
        Tok::RParen => "')'".into(),
        Tok::LBrace => "'{'".into(),
        Tok::RBrace => "'}'".into(),
        Tok::LBracket => "'['".into(),
        Tok::RBracket => "']'".into(),
        Tok::Comma => "','".into(),
        Tok::Semi => "';'".into(),
        Tok::Colon => "':'".into(),
        Tok::Eof => "end of input".into(),
    }
}
