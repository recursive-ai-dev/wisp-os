//! Recursive-descent parser: `Vec<Token>` -> `ast::Program`. Precedence
//! climbing follows the table in `docs/WISP_LANGUAGE.md` exactly, lowest
//! to highest: `or`, `and`, `==`/`!=`, comparisons, `+`/`-`, `*`/`/`/`%`,
//! unary `not`/`-`, then calls/grouping/literals.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::ast::{BinaryOp, Expr, FuncDecl, Program, Stmt, TopItem, UnaryOp};
use super::lexer::{Tok, Token};
use super::WispError;

pub fn parse(tokens: &[Token]) -> Result<Program, WispError> {
    let mut p = Parser { tokens, pos: 0 };
    let mut items = Vec::new();
    while !p.is_eof() {
        if p.check(&Tok::Fn) {
            items.push(TopItem::Func(p.parse_func_decl()?));
        } else {
            items.push(TopItem::Stmt(p.parse_stmt()?));
        }
    }
    Ok(Program { items })
}

struct Parser<'t> {
    tokens: &'t [Token],
    pos: usize,
}

impl<'t> Parser<'t> {
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
        Ok(stmts)
    }

    // ---- statements ----

    fn parse_stmt(&mut self) -> Result<Stmt, WispError> {
        match self.peek() {
            Tok::Let => self.parse_let(),
            Tok::If => self.parse_if(),
            Tok::While => self.parse_while(),
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

    fn parse_let(&mut self) -> Result<Stmt, WispError> {
        let line = self.peek_line();
        self.expect(Tok::Let)?;
        let name = self.expect_ident()?;
        self.expect(Tok::Eq)?;
        let expr = self.parse_expr()?;
        self.expect(Tok::Semi)?;
        Ok(Stmt::Let { name, expr, line })
    }

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
                // This function is only ever entered when the statement
                // starts with `ident [`, so `parse_postfix` always
                // produces an `Expr::Index` here.
                _ => unreachable!("parse_index_assign_or_expr_stmt always parses an Expr::Index"),
            };
        }
        self.expect(Tok::Semi)?;
        Ok(Stmt::ExprStmt(target))
    }

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

    fn parse_while(&mut self) -> Result<Stmt, WispError> {
        let line = self.expect(Tok::While)?.line;
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Stmt::While { cond, body, line })
    }

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
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, WispError> {
        let mut lhs = self.parse_and()?;
        while self.check(&Tok::Or) {
            let line = self.peek_line();
            self.advance();
            let rhs = self.parse_and()?;
            lhs = Expr::Binary {
                op: BinaryOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Expr, WispError> {
        let mut lhs = self.parse_equality()?;
        while self.check(&Tok::And) {
            let line = self.peek_line();
            self.advance();
            let rhs = self.parse_equality()?;
            lhs = Expr::Binary {
                op: BinaryOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            };
        }
        Ok(lhs)
    }

    fn parse_equality(&mut self) -> Result<Expr, WispError> {
        let mut lhs = self.parse_comparison()?;
        loop {
            let op = match self.peek() {
                Tok::EqEq => BinaryOp::Eq,
                Tok::NotEq => BinaryOp::Ne,
                _ => break,
            };
            let line = self.peek_line();
            self.advance();
            let rhs = self.parse_comparison()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            };
        }
        Ok(lhs)
    }

    fn parse_comparison(&mut self) -> Result<Expr, WispError> {
        let mut lhs = self.parse_term()?;
        loop {
            let op = match self.peek() {
                Tok::Lt => BinaryOp::Lt,
                Tok::Le => BinaryOp::Le,
                Tok::Gt => BinaryOp::Gt,
                Tok::Ge => BinaryOp::Ge,
                _ => break,
            };
            let line = self.peek_line();
            self.advance();
            let rhs = self.parse_term()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            };
        }
        Ok(lhs)
    }

    fn parse_term(&mut self) -> Result<Expr, WispError> {
        let mut lhs = self.parse_factor()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinaryOp::Add,
                Tok::Minus => BinaryOp::Sub,
                _ => break,
            };
            let line = self.peek_line();
            self.advance();
            let rhs = self.parse_factor()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            };
        }
        Ok(lhs)
    }

    fn parse_factor(&mut self) -> Result<Expr, WispError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinaryOp::Mul,
                Tok::Slash => BinaryOp::Div,
                Tok::Percent => BinaryOp::Rem,
                _ => break,
            };
            let line = self.peek_line();
            self.advance();
            let rhs = self.parse_unary()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                line,
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, WispError> {
        let line = self.peek_line();
        if self.match_tok(&Tok::Not) {
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
                line,
            });
        }
        if self.match_tok(&Tok::Minus) {
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
                line,
            });
        }
        self.parse_postfix()
    }

    /// Wraps a primary expression in zero or more `[index]` reads, e.g.
    /// `grid[y][x]` or `make_list()[0]`. Binds tighter than unary
    /// `not`/`-` and looser than nothing (it's the tightest-binding
    /// construct in the language, same as function calls).
    fn parse_postfix(&mut self) -> Result<Expr, WispError> {
        let mut expr = self.parse_primary()?;
        while self.check(&Tok::LBracket) {
            let line = self.peek_line();
            self.advance();
            let index = self.parse_expr()?;
            self.expect(Tok::RBracket)?;
            expr = Expr::Index {
                object: Box::new(expr),
                index: Box::new(index),
                line,
            };
        }
        Ok(expr)
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
            other => Err(WispError::new(line, format!("unexpected {}", describe(&other)))),
        }
    }
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
        Tok::Eof => "end of input".into(),
    }
}
