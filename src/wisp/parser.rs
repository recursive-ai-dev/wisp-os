//! Recursive-descent parser: `Vec<Token>` -> `ast::Program`. Precedence
//! climbing follows the table in `docs/WISP_LANGUAGE.md` exactly, lowest
//! to highest: `or`, `and`, `==`/`!=`, comparisons, `+`/`-`, `*`/`/`/`%`,
//! unary `not`/`-`, then calls/grouping/literals (including `[...]` list
//! and `{...}` map literals).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::ast::{BinaryOp, Expr, FuncDecl, Program, Stmt, TopItem, UnaryOp};
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
    let mut p = Parser { tokens, pos: 0, depth: 0 };
    let mut items = Vec::new();
    while !p.is_eof() {
        if p.check(&Tok::Fn) {
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
}

impl<'t> Parser<'t> {
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
        if self.check(&want) {
            Ok(self.advance())
        } else {
            Err(WispError::new(
                line,
                format!("expected {}, found {}", describe(&want), describe(self.peek())),
            ))
        }
    }

    fn expect_ident(&mut self) -> Result<alloc::rc::Rc<String>, WispError> {
        let line = self.peek_line();
        match self.advance().tok {
            Tok::Ident(name) => Ok(name),
            other => Err(WispError::new(
                line,
                format!("expected identifier, found {}", describe(&other)),
            )),
        }
    }

    // ---- top-level items ----

    #[inline(never)]
    fn parse_func_decl(&mut self) -> Result<FuncDecl, WispError> {
        let line = self.peek_line();
        self.expect(Tok::Fn)?;
        let name = self.expect_ident()?;
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
        Ok(FuncDecl {
            name,
            params,
            body,
            line,
        })
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, WispError> {
        self.descend()?;
        self.expect(Tok::LBrace)?;
        let mut stmts = Vec::new();
        while !self.check(&Tok::RBrace) {
            if self.is_eof() {
                return Err(WispError::new(
                    self.peek_line(),
                    "unterminated block, expected '}'",
                ));
            }
            stmts.push(self.parse_stmt()?);
        }
        self.expect(Tok::RBrace)?;
        self.depth -= 1;
        Ok(stmts)
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
                Ok(Stmt::Break { line })
            }
            Tok::Continue => {
                let line = self.peek_line();
                self.advance();
                self.expect(Tok::Semi)?;
                Ok(Stmt::Continue { line })
            }
            Tok::Return => self.parse_return(),
            Tok::Ident(_) if matches!(self.peek_at(1), Tok::Eq) => self.parse_assign(),
            Tok::Ident(_) if matches!(self.peek_at(1), Tok::LBracket) => {
                self.parse_index_assign_or_expr_stmt()
            }
            _ => {
                let expr = self.parse_expr()?;
                self.expect(Tok::Semi)?;
                Ok(Stmt::ExprStmt(expr))
            }
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
        Ok(Stmt::Let { name, expr, line })
    }

    #[inline(never)]
    fn parse_assign(&mut self) -> Result<Stmt, WispError> {
        let line = self.peek_line();
        let name = self.expect_ident()?;
        self.expect(Tok::Eq)?;
        let expr = self.parse_expr()?;
        self.expect(Tok::Semi)?;
        Ok(Stmt::Assign { name, expr, line })
    }

    /// Handles a statement that starts `ident [` -- either a list write
    /// (`name[idx] = value;`, possibly through several chained indices
    /// like `grid[y][x] = value;`) or, if no `=` follows the index chain,
    /// an ordinary expression statement that happens to start with a list
    /// read (e.g. a bare `queue[0];`, which is legal but pointless).
    /// Both start identically -- an identifier followed by one or more
    /// `[...]` -- and only diverge at the token right after that chain,
    /// so this parses the postfix target once and branches on what comes
    /// next, rather than needing unbounded lookahead or backtracking.
    #[inline(never)]
    fn parse_index_assign_or_expr_stmt(&mut self) -> Result<Stmt, WispError> {
        let line = self.peek_line();
        let target = self.parse_postfix()?;
        if self.match_tok(&Tok::Eq) {
            let value = self.parse_expr()?;
            self.expect(Tok::Semi)?;
            return match target {
                Expr::Index { object, index, .. } => Ok(Stmt::IndexAssign {
                    object: *object,
                    index: *index,
                    value,
                    line,
                }),
                Expr::Slice { .. } => Err(WispError::new(
                    line,
                    "cannot assign to a slice (assign one element at a time, or use insert()/remove())",
                )),
                // This function is only ever entered when the statement
                // starts with `ident [`, so `parse_postfix` always
                // produces an `Expr::Index` or `Expr::Slice` here.
                _ => unreachable!("parse_index_assign_or_expr_stmt always parses an Expr::Index or Expr::Slice"),
            };
        }
        self.expect(Tok::Semi)?;
        Ok(Stmt::ExprStmt(target))
    }

    #[inline(never)]
    fn parse_if(&mut self) -> Result<Stmt, WispError> {
        self.expect(Tok::If)?;
        let mut branches = Vec::new();
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        branches.push((cond, body));

        let else_body = loop {
            if self.match_tok(&Tok::Elif) {
                let cond = self.parse_expr()?;
                let body = self.parse_block()?;
                branches.push((cond, body));
                continue;
            }
            if self.match_tok(&Tok::Else) {
                break Some(self.parse_block()?);
            }
            break None;
        };

        Ok(Stmt::If { branches, else_body })
    }

    #[inline(never)]
    fn parse_while(&mut self) -> Result<Stmt, WispError> {
        let line = self.expect(Tok::While)?.line;
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Stmt::While { cond, body, line })
    }

    #[inline(never)]
    fn parse_for(&mut self) -> Result<Stmt, WispError> {
        let line = self.expect(Tok::For)?.line;
        let var = self.expect_ident()?;
        self.expect(Tok::In)?;
        let iter = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Stmt::For { var, iter, body, line })
    }

    #[inline(never)]
    fn parse_return(&mut self) -> Result<Stmt, WispError> {
        self.expect(Tok::Return)?;
        let expr = if self.check(&Tok::Semi) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(Tok::Semi)?;
        Ok(Stmt::Return { expr })
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
        while let Some((op, prec)) = binary_op(self.peek()) {
            if prec < min_prec {
                break;
            }
            let line = self.peek_line();
            self.advance();
            self.descend()?;
            let rhs = self.parse_binary(prec + 1)?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            };
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
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
                line,
            });
        }
        if self.match_tok(&Tok::Minus) {
            self.descend()?;
            let expr = self.parse_unary()?;
            self.depth -= 1;
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
                line,
            });
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
        self.advance();
        self.descend()?;
        let first = if self.check(&Tok::Colon) { None } else { Some(Box::new(self.parse_expr()?)) };
        if self.match_tok(&Tok::Colon) {
            let end = if self.check(&Tok::RBracket) { None } else { Some(Box::new(self.parse_expr()?)) };
            self.expect(Tok::RBracket)?;
            Ok(Expr::Slice { object: Box::new(object), start: first, end, line })
        } else {
            self.expect(Tok::RBracket)?;
            Ok(Expr::Index {
                object: Box::new(object),
                index: first.expect("an index expression was parsed: the token after '[' was not ':'"),
                line,
            })
        }
    }

    /// A call on something that is not a bare name -- the result of an
    /// index, a call or a parenthesized expression -- after its `(`.
    /// (`name(` was taken by `parse_primary`.) Outlined so its locals are
    /// not paid at every level of expression nesting.
    #[inline(never)]
    fn parse_call_suffix(&mut self, callee: Expr, line: u32) -> Result<Expr, WispError> {
        self.descend()?;
        let args = self.parse_call_args()?;
        Ok(Expr::CallValue { callee: Box::new(callee), args, line })
    }

    /// A call's arguments, after its `(`, through its `)`. Outlined from
    /// `parse_primary` so its locals are not paid at every level of
    /// expression nesting.
    #[inline(never)]
    fn parse_call_args(&mut self) -> Result<Vec<Expr>, WispError> {
        let mut args = Vec::new();
        if !self.check(&Tok::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if self.match_tok(&Tok::Comma) {
                    continue;
                }
                break;
            }
        }
        self.expect(Tok::RParen)?;
        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr, WispError> {
        let line = self.peek_line();
        match self.advance().tok {
            Tok::Int(v) => Ok(Expr::IntLit(v)),
            Tok::Str(s) => Ok(Expr::StrLit(s)),
            Tok::True => Ok(Expr::BoolLit(true)),
            Tok::False => Ok(Expr::BoolLit(false)),
            Tok::Nil => Ok(Expr::NilLit),
            Tok::Ident(name) => {
                if self.match_tok(&Tok::LParen) {
                    let args = self.parse_call_args()?;
                    Ok(Expr::Call { name, args, line })
                } else {
                    Ok(Expr::Var { name, line })
                }
            }
            Tok::LParen => {
                let expr = self.parse_expr()?;
                self.expect(Tok::RParen)?;
                Ok(expr)
            }
            Tok::LBracket => {
                let mut items = Vec::new();
                if !self.check(&Tok::RBracket) {
                    loop {
                        items.push(self.parse_expr()?);
                        if self.match_tok(&Tok::Comma) {
                            continue;
                        }
                        break;
                    }
                }
                self.expect(Tok::RBracket)?;
                Ok(Expr::ListLit { items, line })
            }
            // Only reached where an operand is expected, so a `{` here is
            // never a block: a block's `{` comes after a complete condition
            // or a `)`, where the expression parser has already stopped.
            Tok::LBrace => {
                let entries = self.parse_map_entries()?;
                Ok(Expr::MapLit { entries, line })
            }
            other => Err(WispError::new(line, format!("unexpected {}", describe(&other)))),
        }
    }

    /// The entries of a map literal, after its `{`, through its `}`.
    /// Outlined from `parse_primary` so that its locals are not paid at
    /// every level of expression nesting.
    #[inline(never)]
    fn parse_map_entries(&mut self) -> Result<Vec<(Expr, Expr)>, WispError> {
        let mut entries = Vec::new();
        if !self.check(&Tok::RBrace) {
            loop {
                let key = self.parse_expr()?;
                if !self.match_tok(&Tok::Colon) {
                    return Err(WispError::new(
                        self.peek_line(),
                        format!("expected ':' after a map key, found {}", describe(self.peek())),
                    ));
                }
                let value = self.parse_expr()?;
                entries.push((key, value));
                if self.match_tok(&Tok::Comma) {
                    continue;
                }
                break;
            }
        }
        self.expect(Tok::RBrace)?;
        Ok(entries)
    }
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
