//! Recursive-descent + precedence-climbing parser for `CodeSceneScript` (`cs1`).
//! Enforces hard `max_ast_depth` (`64`) on every recursive construct.

use crate::lang::ast::{AssignOp, BinOp, Expr, FnDecl, LValue, Program, Stmt, TopItem, UnaryOp};
use crate::lang::lexer::{Token, TokenKind};
use crate::lang::limits::VmBudgets;
use crate::model::{CompileDiagnostic, DiagnosticStage, Span};

static EOF_TOKEN: Token = Token {
    kind: TokenKind::Eof,
    span: Span { line: 1, col: 1 },
    after_newline: false,
};

pub fn parse(tokens: &[Token], budgets: &VmBudgets) -> Result<Program, CompileDiagnostic> {
    if tokens.is_empty() {
        return Err(CompileDiagnostic::new(
            DiagnosticStage::Parse,
            "E_PARSE_EMPTY",
            "empty token stream",
            None,
        ));
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        depth: 0,
        max_depth: budgets.max_ast_depth,
    };
    parser.parse_program()
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    depth: usize,
    max_depth: usize,
}

impl<'a> Parser<'a> {
    fn cur(&self) -> &'a Token {
        self.tokens.get(self.pos).unwrap_or(&EOF_TOKEN)
    }

    fn peek(&self) -> &'a TokenKind {
        &self.cur().kind
    }

    fn span(&self) -> Span {
        self.cur().span
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn advance(&mut self) -> &'a Token {
        let tok = self.cur();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.peek() == kind {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind, label: &str) -> Result<&'a Token, CompileDiagnostic> {
        if self.peek() == kind {
            Ok(self.advance())
        } else {
            Err(CompileDiagnostic::new(
                DiagnosticStage::Parse,
                "E_PARSE_EXPECTED",
                format!("expected {label}, found {:?}", self.peek()),
                Some(self.span()),
            ))
        }
    }

    fn expect_ident(&mut self, label: &str) -> Result<(String, Span), CompileDiagnostic> {
        let tok = self.cur();
        if let TokenKind::Ident(name) = &tok.kind {
            let out = (name.clone(), tok.span);
            self.advance();
            Ok(out)
        } else {
            Err(CompileDiagnostic::new(
                DiagnosticStage::Parse,
                "E_PARSE_EXPECTED",
                format!("expected {label}, found {:?}", tok.kind),
                Some(tok.span),
            ))
        }
    }

    fn enter_depth(&mut self, span: Span) -> Result<(), CompileDiagnostic> {
        if self.depth >= self.max_depth {
            return Err(CompileDiagnostic::new(
                DiagnosticStage::Parse,
                "E_AST_DEPTH_EXCEEDED",
                format!("AST nesting depth exceeds limit {}", self.max_depth),
                Some(span),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    fn exit_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn consume_optional_semi(&mut self) {
        while self.eat(&TokenKind::Semicolon) {}
    }

    fn parse_program(&mut self) -> Result<Program, CompileDiagnostic> {
        let mut items = Vec::new();
        while !self.at_eof() {
            self.consume_optional_semi();
            if self.at_eof() {
                break;
            }
            match self.peek() {
                TokenKind::Fn => items.push(TopItem::Fn(self.parse_fn()?)),
                TokenKind::Let => {
                    let span = self.span();
                    self.advance();
                    let (name, _) = self.expect_ident("global variable name")?;
                    self.expect(&TokenKind::Eq, "`=`")?;
                    let value = self.parse_expr()?;
                    self.consume_optional_semi();
                    items.push(TopItem::GlobalLet { name, value, span });
                }
                _ => {
                    return Err(CompileDiagnostic::new(
                        DiagnosticStage::Parse,
                        "E_PARSE_TOP_LEVEL",
                        format!(
                            "expected `fn` or `let` at top level, found {:?}",
                            self.peek()
                        ),
                        Some(self.span()),
                    ));
                }
            }
        }
        Ok(Program { items })
    }

    fn parse_fn(&mut self) -> Result<FnDecl, CompileDiagnostic> {
        let span = self.span();
        self.expect(&TokenKind::Fn, "`fn`")?;
        let (name, _) = self.expect_ident("function name")?;
        self.expect(&TokenKind::LParen, "`(`")?;
        let mut params = Vec::new();
        if !matches!(self.peek(), TokenKind::RParen) {
            loop {
                let (p, p_span) = self.expect_ident("parameter name")?;
                if params.contains(&p) {
                    return Err(CompileDiagnostic::new(
                        DiagnosticStage::Parse,
                        "E_DUPLICATE_PARAM",
                        format!("duplicate parameter `{p}` in function `{name}`"),
                        Some(p_span),
                    ));
                }
                params.push(p);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if matches!(self.peek(), TokenKind::RParen) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        let body = self.parse_block()?;
        Ok(FnDecl {
            name,
            params,
            body,
            span,
        })
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, CompileDiagnostic> {
        let span = self.span();
        self.enter_depth(span)?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            if self.eat(&TokenKind::Semicolon) {
                continue;
            }
            stmts.push(self.parse_stmt()?);
        }
        self.expect(&TokenKind::RBrace, "`}`")?;
        self.exit_depth();
        Ok(stmts)
    }
}

impl<'a> Parser<'a> {
    fn parse_stmt(&mut self) -> Result<Stmt, CompileDiagnostic> {
        let span = self.span();
        self.enter_depth(span)?;
        let res = match self.peek() {
            TokenKind::Let => self.parse_let_stmt(),
            TokenKind::If => self.parse_if_stmt(),
            TokenKind::While => self.parse_while_stmt(),
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::Break => {
                self.advance();
                self.consume_optional_semi();
                Ok(Stmt::Break { span })
            }
            TokenKind::Continue => {
                self.advance();
                self.consume_optional_semi();
                Ok(Stmt::Continue { span })
            }
            TokenKind::Return => {
                self.advance();
                let value = if matches!(
                    self.peek(),
                    TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Eof
                ) || self.cur().after_newline
                {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                self.consume_optional_semi();
                Ok(Stmt::Return { value, span })
            }
            TokenKind::LBrace => {
                let stmts = self.parse_block()?;
                Ok(Stmt::Block { stmts, span })
            }
            _ => self.parse_expr_or_assign_stmt(),
        };
        self.exit_depth();
        res
    }

    fn parse_let_stmt(&mut self) -> Result<Stmt, CompileDiagnostic> {
        let span = self.span();
        self.advance();
        let (name, _) = self.expect_ident("variable name")?;
        self.expect(&TokenKind::Eq, "`=`")?;
        let value = self.parse_expr()?;
        self.consume_optional_semi();
        Ok(Stmt::Let { name, value, span })
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, CompileDiagnostic> {
        let span = self.span();
        self.advance();
        let cond = self.parse_expr()?;
        let then_block = self.parse_block()?;
        let else_block = if self.eat(&TokenKind::Else) {
            if matches!(self.peek(), TokenKind::If) {
                let nested = self.parse_stmt()?;
                Some(vec![nested])
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };
        Ok(Stmt::If {
            cond,
            then_block,
            else_block,
            span,
        })
    }

    fn parse_while_stmt(&mut self) -> Result<Stmt, CompileDiagnostic> {
        let span = self.span();
        self.advance();
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        Ok(Stmt::While { cond, body, span })
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, CompileDiagnostic> {
        let span = self.span();
        self.advance();
        let (var, _) = self.expect_ident("loop variable")?;
        self.expect(&TokenKind::In, "`in`")?;
        let first = self.parse_expr()?;
        if self.eat(&TokenKind::DotDot) {
            let end = self.parse_expr()?;
            let body = self.parse_block()?;
            Ok(Stmt::ForRange {
                var,
                start: first,
                end,
                inclusive: false,
                body,
                span,
            })
        } else if self.eat(&TokenKind::DotDotEq) {
            let end = self.parse_expr()?;
            let body = self.parse_block()?;
            Ok(Stmt::ForRange {
                var,
                start: first,
                end,
                inclusive: true,
                body,
                span,
            })
        } else {
            let body = self.parse_block()?;
            Ok(Stmt::ForEach {
                var,
                iter: first,
                body,
                span,
            })
        }
    }

    fn parse_expr_or_assign_stmt(&mut self) -> Result<Stmt, CompileDiagnostic> {
        let span = self.span();
        let lhs = self.parse_expr()?;
        let assign_op = match self.peek() {
            TokenKind::Eq => Some(AssignOp::Assign),
            TokenKind::PlusEq => Some(AssignOp::AddAssign),
            TokenKind::MinusEq => Some(AssignOp::SubAssign),
            TokenKind::StarEq => Some(AssignOp::MulAssign),
            TokenKind::SlashEq => Some(AssignOp::DivAssign),
            TokenKind::PercentEq => Some(AssignOp::ModAssign),
            _ => None,
        };
        if let Some(op) = assign_op {
            let target = match lhs {
                Expr::Ident(name, s) => LValue::Var { name, span: s },
                Expr::Field { base, field, span: s } => LValue::Field { base, field, span: s },
                Expr::Index { base, index, span: s } => LValue::Index { base, index, span: s },
                other => {
                    return Err(CompileDiagnostic::new(
                        DiagnosticStage::Parse,
                        "E_INVALID_LVALUE",
                        "left-hand side of assignment must be a variable, field, or index expression",
                        Some(other.span()),
                    ));
                }
            };
            self.advance();
            let value = self.parse_expr()?;
            self.consume_optional_semi();
            Ok(Stmt::Assign {
                target,
                op,
                value,
                span,
            })
        } else {
            self.consume_optional_semi();
            Ok(Stmt::Expr { expr: lhs, span })
        }
    }
}

impl<'a> Parser<'a> {
    fn parse_expr(&mut self) -> Result<Expr, CompileDiagnostic> {
        self.parse_prec(1)
    }

    fn bin_op_prec(kind: &TokenKind) -> Option<(u8, BinOp)> {
        match kind {
            TokenKind::OrOr => Some((1, BinOp::Or)),
            TokenKind::AndAnd => Some((2, BinOp::And)),
            TokenKind::EqEq => Some((3, BinOp::Eq)),
            TokenKind::BangEq => Some((3, BinOp::Ne)),
            TokenKind::Lt => Some((4, BinOp::Lt)),
            TokenKind::LtEq => Some((4, BinOp::Le)),
            TokenKind::Gt => Some((4, BinOp::Gt)),
            TokenKind::GtEq => Some((4, BinOp::Ge)),
            TokenKind::Plus => Some((5, BinOp::Add)),
            TokenKind::Minus => Some((5, BinOp::Sub)),
            TokenKind::Star => Some((6, BinOp::Mul)),
            TokenKind::Slash => Some((6, BinOp::Div)),
            TokenKind::Percent => Some((6, BinOp::Mod)),
            _ => None,
        }
    }

    fn parse_prec(&mut self, min_prec: u8) -> Result<Expr, CompileDiagnostic> {
        let mut lhs = self.parse_unary()?;
        let mut chain_depth = 0usize;
        while let Some((prec, op)) = Self::bin_op_prec(self.peek()) {
            if prec < min_prec {
                break;
            }
            let span = self.span();
            if let Err(err) = self.enter_depth(span) {
                self.depth = self.depth.saturating_sub(chain_depth);
                return Err(err);
            }
            chain_depth += 1;
            self.advance();
            let rhs = match self.parse_prec(prec + 1) {
                Ok(expr) => expr,
                Err(err) => {
                    self.depth = self.depth.saturating_sub(chain_depth);
                    return Err(err);
                }
            };
            lhs = Expr::Binary {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
                span,
            };
        }
        self.depth = self.depth.saturating_sub(chain_depth);
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, CompileDiagnostic> {
        let span = self.span();
        match self.peek() {
            TokenKind::Minus => {
                self.enter_depth(span)?;
                self.advance();
                let operand = self.parse_unary()?;
                self.exit_depth();
                Ok(Expr::Unary {
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                    span,
                })
            }
            TokenKind::Bang => {
                self.enter_depth(span)?;
                self.advance();
                let operand = self.parse_unary()?;
                self.exit_depth();
                Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                    span,
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, CompileDiagnostic> {
        let mut expr = self.parse_primary()?;
        let mut chain_depth = 0usize;
        let res: Result<Expr, CompileDiagnostic> = (|| {
            loop {
                match self.peek() {
                    TokenKind::LParen => {
                        let span = self.span();
                        let callee = match expr {
                            Expr::Ident(name, _) => name,
                            _ => {
                                return Err(CompileDiagnostic::new(
                                    DiagnosticStage::Parse,
                                    "E_INVALID_CALL",
                                    "only named functions or methods can be called",
                                    Some(span),
                                ));
                            }
                        };
                        let args = self.parse_arg_list()?;
                        expr = Expr::Call { callee, args, span };
                    }
                    TokenKind::Dot => {
                        let span = self.span();
                        self.enter_depth(span)?;
                        chain_depth += 1;
                        self.advance();
                        let (field, _) = self.expect_ident("field or method name")?;
                        if matches!(self.peek(), TokenKind::LParen) {
                            let args = self.parse_arg_list()?;
                            expr = Expr::MethodCall {
                                receiver: Box::new(expr),
                                method: field,
                                args,
                                span,
                            };
                        } else {
                            expr = Expr::Field {
                                base: Box::new(expr),
                                field,
                                span,
                            };
                        }
                    }
                    TokenKind::LBracket => {
                        let span = self.span();
                        self.enter_depth(span)?;
                        chain_depth += 1;
                        self.advance();
                        let index = self.parse_expr()?;
                        self.expect(&TokenKind::RBracket, "`]`")?;
                        expr = Expr::Index {
                            base: Box::new(expr),
                            index: Box::new(index),
                            span,
                        };
                    }
                    _ => break,
                }
            }
            Ok(expr)
        })();
        self.depth = self.depth.saturating_sub(chain_depth);
        res
    }

    fn parse_arg_list(&mut self) -> Result<Vec<Expr>, CompileDiagnostic> {
        let span = self.span();
        self.enter_depth(span)?;
        self.expect(&TokenKind::LParen, "`(`")?;
        let mut args = Vec::new();
        if !matches!(self.peek(), TokenKind::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if matches!(self.peek(), TokenKind::RParen) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        self.exit_depth();
        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr, CompileDiagnostic> {
        let span = self.span();
        match self.peek().clone() {
            TokenKind::Null => {
                self.advance();
                Ok(Expr::Null(span))
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr::Bool(true, span))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr::Bool(false, span))
            }
            TokenKind::Num(n) => {
                self.advance();
                Ok(Expr::Num(n, span))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr::Str(s, span))
            }
            TokenKind::Ident(name) => {
                self.advance();
                Ok(Expr::Ident(name, span))
            }
            TokenKind::LParen => {
                self.enter_depth(span)?;
                self.advance();
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::RParen, "`)`")?;
                self.exit_depth();
                Ok(inner)
            }
            TokenKind::LBracket => {
                self.enter_depth(span)?;
                self.advance();
                let mut elems = Vec::new();
                if !matches!(self.peek(), TokenKind::RBracket) {
                    loop {
                        elems.push(self.parse_expr()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                        if matches!(self.peek(), TokenKind::RBracket) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RBracket, "`]`")?;
                self.exit_depth();
                Ok(Expr::List(elems, span))
            }
            TokenKind::LBrace => {
                self.enter_depth(span)?;
                self.advance();
                let mut pairs = Vec::new();
                if !matches!(self.peek(), TokenKind::RBrace) {
                    loop {
                        let key = match self.peek().clone() {
                            TokenKind::Ident(k) => {
                                self.advance();
                                k
                            }
                            TokenKind::Str(s) => {
                                self.advance();
                                s.to_string()
                            }
                            other => {
                                self.exit_depth();
                                return Err(CompileDiagnostic::new(
                                    DiagnosticStage::Parse,
                                    "E_PARSE_MAP_KEY",
                                    format!("expected map key identifier or string, found {other:?}"),
                                    Some(self.span()),
                                ));
                            }
                        };
                        self.expect(&TokenKind::Colon, "`:`")?;
                        let val = self.parse_expr()?;
                        pairs.push((key, val));
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                        if matches!(self.peek(), TokenKind::RBrace) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RBrace, "`}`")?;
                self.exit_depth();
                Ok(Expr::Map(pairs, span))
            }
            other => Err(CompileDiagnostic::new(
                DiagnosticStage::Parse,
                "E_PARSE_EXPR",
                format!("unexpected token `{other:?}` in expression"),
                Some(span),
            )),
        }
    }
}
