//! Recursive-descent Python 3 parser supporting decorators, type annotations, comprehensions,
//! f-strings, classes, try/except/finally, slicing, lambda, and tuple unpacking.

use crate::ast::{
    BinOp, BoolOp, CallArg, CmpOp, Comprehension, ExceptHandler, Expr, ExprKind, FStringPart,
    Param, Params, Program, SlicePart, Stmt, StmtKind, Target, UnaryOp,
};
use crate::error::PyError;
use crate::lexer::{Token, TokenKind, tokenize, unescape_python_str};

pub fn parse_source(file: &str, src: &str) -> Result<Program, PyError> {
    let tokens = tokenize(file, src)?;
    let mut parser = Parser {
        file: file.to_string(),
        tokens,
        pos: 0,
    };
    parser.parse_program()
}

pub fn parse_expr_str(file: &str, line_offset: usize, src: &str) -> Result<Expr, PyError> {
    let mut tokens = tokenize(file, src)?;
    for t in &mut tokens {
        t.line = line_offset;
    }
    let mut parser = Parser {
        file: file.to_string(),
        tokens,
        pos: 0,
    };
    let expr = parser.parse_expr()?;
    parser.skip_newlines();
    if !matches!(parser.peek_kind(), TokenKind::Eof) {
        return Err(parser.err("unexpected trailing token in expression"));
    }
    Ok(expr)
}

struct Parser {
    file: String,
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .unwrap_or_else(|| self.tokens.last().expect("EOF token"))
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn peek_kind_at(&self, offset: usize) -> Option<&TokenKind> {
        self.tokens.get(self.pos + offset).map(|t| &t.kind)
    }

    fn bump(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn err(&self, msg: impl Into<String>) -> PyError {
        let t = self.peek();
        PyError::syntax(&self.file, t.line, t.col, msg)
    }

    fn expect(&mut self, expected: &TokenKind) -> Result<Token, PyError> {
        if self.peek_kind() == expected {
            Ok(self.bump())
        } else {
            Err(self.err(format!(
                "expected {:?}, found {:?}",
                expected,
                self.peek_kind()
            )))
        }
    }

    fn expect_ident(&mut self) -> Result<String, PyError> {
        match self.peek_kind().clone() {
            TokenKind::Ident(s) => {
                self.bump();
                Ok(s)
            }
            other => Err(self.err(format!("expected identifier, found {other:?}"))),
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.bump();
        }
    }

    fn parse_program(&mut self) -> Result<Program, PyError> {
        let mut body = Vec::new();
        self.skip_newlines();
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            body.extend(self.parse_stmt()?);
            self.skip_newlines();
        }
        Ok(Program { body })
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, PyError> {
        if matches!(self.peek_kind(), TokenKind::Newline) {
            self.skip_newlines();
            self.expect(&TokenKind::Indent)?;
            let mut stmts = Vec::new();
            self.skip_newlines();
            while !matches!(self.peek_kind(), TokenKind::Dedent | TokenKind::Eof) {
                stmts.extend(self.parse_stmt()?);
                self.skip_newlines();
            }
            self.expect(&TokenKind::Dedent)?;
            Ok(stmts)
        } else {
            // Single-line suite: `if cond: return 1`
            self.parse_simple_stmt_line()
        }
    }

    fn parse_stmt(&mut self) -> Result<Vec<Stmt>, PyError> {
        match self.peek_kind() {
            TokenKind::At => Ok(vec![self.parse_decorated()?]),
            TokenKind::Def => Ok(vec![self.parse_funcdef(Vec::new())?]),
            TokenKind::Class => Ok(vec![self.parse_classdef(Vec::new())?]),
            TokenKind::If => Ok(vec![self.parse_if()?]),
            TokenKind::While => Ok(vec![self.parse_while()?]),
            TokenKind::For => Ok(vec![self.parse_for()?]),
            TokenKind::Try => Ok(vec![self.parse_try()?]),
            _ => self.parse_simple_stmt_line(),
        }
    }

    fn parse_simple_stmt_line(&mut self) -> Result<Vec<Stmt>, PyError> {
        let mut stmts = Vec::new();
        stmts.push(self.parse_simple_stmt()?);
        while matches!(self.peek_kind(), TokenKind::Semi) {
            self.bump();
            if matches!(
                self.peek_kind(),
                TokenKind::Newline | TokenKind::Eof | TokenKind::Dedent
            ) {
                break;
            }
            stmts.push(self.parse_simple_stmt()?);
        }
        if matches!(self.peek_kind(), TokenKind::Newline) {
            self.bump();
        }
        Ok(stmts)
    }

    fn parse_decorated(&mut self) -> Result<Stmt, PyError> {
        let mut decorators = Vec::new();
        while matches!(self.peek_kind(), TokenKind::At) {
            self.bump();
            let expr = self.parse_expr()?;
            decorators.push(expr);
            self.expect(&TokenKind::Newline)?;
            self.skip_newlines();
        }
        match self.peek_kind() {
            TokenKind::Def => self.parse_funcdef(decorators),
            TokenKind::Class => self.parse_classdef(decorators),
            _ => Err(self.err("decorator must be followed by `def` or `class`")),
        }
    }

    fn parse_funcdef(&mut self, decorators: Vec<Expr>) -> Result<Stmt, PyError> {
        let line = self.peek().line;
        self.expect(&TokenKind::Def)?;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params(TokenKind::RParen)?;
        self.expect(&TokenKind::RParen)?;
        if matches!(self.peek_kind(), TokenKind::Arrow) {
            self.bump();
            let _ret_ann = self.parse_expr()?;
        }
        self.expect(&TokenKind::Colon)?;
        let body = self.parse_block()?;
        Ok(Stmt {
            line,
            kind: StmtKind::FuncDef {
                name,
                params,
                decorators,
                body,
            },
        })
    }

    fn parse_params(&mut self, end_tok: TokenKind) -> Result<Params, PyError> {
        let mut params = Params::default();
        while self.peek_kind() != &end_tok && !matches!(self.peek_kind(), TokenKind::Eof) {
            match self.peek_kind() {
                TokenKind::DoubleStar => {
                    self.bump();
                    let name = self.expect_ident()?;
                    if matches!(self.peek_kind(), TokenKind::Colon) && end_tok == TokenKind::RParen {
                        self.bump();
                        let _ann = self.parse_expr()?;
                    }
                    params.kwarg = Some(name);
                }
                TokenKind::Star => {
                    self.bump();
                    if matches!(self.peek_kind(), TokenKind::Comma) {
                        // Bare `*` keyword-only separator — accept silently
                        self.bump();
                        continue;
                    }
                    let name = self.expect_ident()?;
                    if matches!(self.peek_kind(), TokenKind::Colon) && end_tok == TokenKind::RParen {
                        self.bump();
                        let _ann = self.parse_expr()?;
                    }
                    params.vararg = Some(name);
                }
                TokenKind::Slash => {
                    // Positional-only `/` separator
                    self.bump();
                }
                _ => {
                    let name = self.expect_ident()?;
                    if matches!(self.peek_kind(), TokenKind::Colon) && end_tok == TokenKind::RParen {
                        self.bump();
                        let _ann = self.parse_expr()?;
                    }
                    let default = if matches!(self.peek_kind(), TokenKind::Eq) {
                        self.bump();
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    params.positional.push(Param { name, default });
                }
            }
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
            } else {
                break;
            }
        }
        Ok(params)
    }

    fn parse_classdef(&mut self, decorators: Vec<Expr>) -> Result<Stmt, PyError> {
        let line = self.peek().line;
        self.expect(&TokenKind::Class)?;
        let name = self.expect_ident()?;
        let mut bases = Vec::new();
        if matches!(self.peek_kind(), TokenKind::LParen) {
            self.bump();
            while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
                bases.push(self.parse_expr()?);
                if matches!(self.peek_kind(), TokenKind::Comma) {
                    self.bump();
                } else {
                    break;
                }
            }
            self.expect(&TokenKind::RParen)?;
        }
        self.expect(&TokenKind::Colon)?;
        let body = self.parse_block()?;
        Ok(Stmt {
            line,
            kind: StmtKind::ClassDef {
                name,
                bases,
                decorators,
                body,
            },
        })
    }

    fn parse_if(&mut self) -> Result<Stmt, PyError> {
        let line = self.peek().line;
        self.expect(&TokenKind::If)?;
        let test = self.parse_expr()?;
        self.expect(&TokenKind::Colon)?;
        let body = self.parse_block()?;
        let mut elifs = Vec::new();
        self.skip_newlines();
        while matches!(self.peek_kind(), TokenKind::Elif) {
            self.bump();
            let e_test = self.parse_expr()?;
            self.expect(&TokenKind::Colon)?;
            let e_body = self.parse_block()?;
            elifs.push((e_test, e_body));
            self.skip_newlines();
        }
        let orelse = if matches!(self.peek_kind(), TokenKind::Else) {
            self.bump();
            self.expect(&TokenKind::Colon)?;
            self.parse_block()?
        } else {
            Vec::new()
        };
        Ok(Stmt {
            line,
            kind: StmtKind::If {
                test,
                body,
                elifs,
                orelse,
            },
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, PyError> {
        let line = self.peek().line;
        self.expect(&TokenKind::While)?;
        let test = self.parse_expr()?;
        self.expect(&TokenKind::Colon)?;
        let body = self.parse_block()?;
        self.skip_newlines();
        let orelse = if matches!(self.peek_kind(), TokenKind::Else) {
            self.bump();
            self.expect(&TokenKind::Colon)?;
            self.parse_block()?
        } else {
            Vec::new()
        };
        Ok(Stmt {
            line,
            kind: StmtKind::While { test, body, orelse },
        })
    }

    fn parse_for(&mut self) -> Result<Stmt, PyError> {
        let line = self.peek().line;
        self.expect(&TokenKind::For)?;
        let target = self.parse_target_list()?;
        self.expect(&TokenKind::In)?;
        let iter = self.parse_expr_or_tuple()?;
        self.expect(&TokenKind::Colon)?;
        let body = self.parse_block()?;
        self.skip_newlines();
        let orelse = if matches!(self.peek_kind(), TokenKind::Else) {
            self.bump();
            self.expect(&TokenKind::Colon)?;
            self.parse_block()?
        } else {
            Vec::new()
        };
        Ok(Stmt {
            line,
            kind: StmtKind::For {
                target,
                iter,
                body,
                orelse,
            },
        })
    }

    fn parse_try(&mut self) -> Result<Stmt, PyError> {
        let line = self.peek().line;
        self.expect(&TokenKind::Try)?;
        self.expect(&TokenKind::Colon)?;
        let body = self.parse_block()?;
        let mut handlers = Vec::new();
        self.skip_newlines();
        while matches!(self.peek_kind(), TokenKind::Except) {
            let h_line = self.peek().line;
            self.bump();
            let (exc_type, name) = if matches!(self.peek_kind(), TokenKind::Colon) {
                (None, None)
            } else {
                let ty = self.parse_expr()?;
                let alias = if matches!(self.peek_kind(), TokenKind::As) {
                    self.bump();
                    Some(self.expect_ident()?)
                } else {
                    None
                };
                (Some(ty), alias)
            };
            self.expect(&TokenKind::Colon)?;
            let h_body = self.parse_block()?;
            handlers.push(ExceptHandler {
                exc_type,
                name,
                body: h_body,
                line: h_line,
            });
            self.skip_newlines();
        }
        let orelse = if matches!(self.peek_kind(), TokenKind::Else) {
            self.bump();
            self.expect(&TokenKind::Colon)?;
            let b = self.parse_block()?;
            self.skip_newlines();
            b
        } else {
            Vec::new()
        };
        let finalbody = if matches!(self.peek_kind(), TokenKind::Finally) {
            self.bump();
            self.expect(&TokenKind::Colon)?;
            self.parse_block()?
        } else {
            Vec::new()
        };
        if handlers.is_empty() && finalbody.is_empty() {
            return Err(self.err("`try` statement must have at least one `except` or `finally`"));
        }
        Ok(Stmt {
            line,
            kind: StmtKind::Try {
                body,
                handlers,
                orelse,
                finalbody,
            },
        })
    }

    fn parse_simple_stmt(&mut self) -> Result<Stmt, PyError> {
        let line = self.peek().line;
        match self.peek_kind() {
            TokenKind::Pass => {
                self.bump();
                Ok(Stmt {
                    line,
                    kind: StmtKind::Pass,
                })
            }
            TokenKind::Break => {
                self.bump();
                Ok(Stmt {
                    line,
                    kind: StmtKind::Break,
                })
            }
            TokenKind::Continue => {
                self.bump();
                Ok(Stmt {
                    line,
                    kind: StmtKind::Continue,
                })
            }
            TokenKind::Return => {
                self.bump();
                let val = if matches!(
                    self.peek_kind(),
                    TokenKind::Newline | TokenKind::Semi | TokenKind::Eof | TokenKind::Dedent
                ) {
                    None
                } else {
                    Some(self.parse_expr_or_tuple()?)
                };
                Ok(Stmt {
                    line,
                    kind: StmtKind::Return(val),
                })
            }
            TokenKind::Raise => {
                self.bump();
                let exc = if matches!(
                    self.peek_kind(),
                    TokenKind::Newline | TokenKind::Semi | TokenKind::Eof | TokenKind::Dedent
                ) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                Ok(Stmt {
                    line,
                    kind: StmtKind::Raise(exc),
                })
            }
            TokenKind::Assert => {
                self.bump();
                let test = self.parse_expr()?;
                let msg = if matches!(self.peek_kind(), TokenKind::Comma) {
                    self.bump();
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                Ok(Stmt {
                    line,
                    kind: StmtKind::Assert { test, msg },
                })
            }
            TokenKind::Del => {
                self.bump();
                let mut targets = Vec::new();
                loop {
                    let expr = self.parse_expr()?;
                    targets.push(expr_to_target(expr, &self.file)?);
                    if matches!(self.peek_kind(), TokenKind::Comma) {
                        self.bump();
                        if matches!(
                            self.peek_kind(),
                            TokenKind::Newline | TokenKind::Semi | TokenKind::Eof
                        ) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                Ok(Stmt {
                    line,
                    kind: StmtKind::Delete(targets),
                })
            }
            TokenKind::Global => {
                self.bump();
                let mut names = vec![self.expect_ident()?];
                while matches!(self.peek_kind(), TokenKind::Comma) {
                    self.bump();
                    names.push(self.expect_ident()?);
                }
                Ok(Stmt {
                    line,
                    kind: StmtKind::Global(names),
                })
            }
            TokenKind::Nonlocal => {
                self.bump();
                let mut names = vec![self.expect_ident()?];
                while matches!(self.peek_kind(), TokenKind::Comma) {
                    self.bump();
                    names.push(self.expect_ident()?);
                }
                Ok(Stmt {
                    line,
                    kind: StmtKind::Nonlocal(names),
                })
            }
            TokenKind::Import => self.parse_import(line),
            TokenKind::From => self.parse_from_import(line),
            _ => self.parse_assign_or_expr(line),
        }
    }

    fn parse_dotted_name(&mut self) -> Result<String, PyError> {
        let mut s = self.expect_ident()?;
        while matches!(self.peek_kind(), TokenKind::Dot) {
            self.bump();
            s.push('.');
            s.push_str(&self.expect_ident()?);
        }
        Ok(s)
    }

    fn parse_import(&mut self, line: usize) -> Result<Stmt, PyError> {
        self.expect(&TokenKind::Import)?;
        let mut names = Vec::new();
        loop {
            let mod_name = self.parse_dotted_name()?;
            let alias = if matches!(self.peek_kind(), TokenKind::As) {
                self.bump();
                Some(self.expect_ident()?)
            } else {
                None
            };
            names.push((mod_name, alias));
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
            } else {
                break;
            }
        }
        Ok(Stmt {
            line,
            kind: StmtKind::Import(names),
        })
    }

    fn parse_from_import(&mut self, line: usize) -> Result<Stmt, PyError> {
        self.expect(&TokenKind::From)?;
        let module = self.parse_dotted_name()?;
        self.expect(&TokenKind::Import)?;
        if matches!(self.peek_kind(), TokenKind::Star) {
            self.bump();
            return Ok(Stmt {
                line,
                kind: StmtKind::FromImport {
                    module,
                    names: Vec::new(),
                    star: true,
                },
            });
        }
        let parenthesized = matches!(self.peek_kind(), TokenKind::LParen);
        if parenthesized {
            self.bump();
        }
        let mut names = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            if parenthesized && matches!(self.peek_kind(), TokenKind::RParen) {
                break;
            }
            let name = self.expect_ident()?;
            let alias = if matches!(self.peek_kind(), TokenKind::As) {
                self.bump();
                Some(self.expect_ident()?)
            } else {
                None
            };
            names.push((name, alias));
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
                if parenthesized && matches!(self.peek_kind(), TokenKind::RParen) {
                    break;
                }
            } else {
                break;
            }
        }
        if parenthesized {
            self.expect(&TokenKind::RParen)?;
        }
        Ok(Stmt {
            line,
            kind: StmtKind::FromImport {
                module,
                names,
                star: false,
            },
        })
    }

    fn parse_assign_or_expr(&mut self, line: usize) -> Result<Stmt, PyError> {
        let first = self.parse_expr_or_tuple()?;

        // Type annotation assignment: `x: int = 0` or `x: int`
        if matches!(self.peek_kind(), TokenKind::Colon) {
            self.bump();
            let _ann = self.parse_expr()?;
            if matches!(self.peek_kind(), TokenKind::Eq) {
                self.bump();
                let value = self.parse_expr_or_tuple()?;
                let target = expr_to_target(first, &self.file)?;
                return Ok(Stmt {
                    line,
                    kind: StmtKind::Assign {
                        targets: vec![target],
                        value,
                    },
                });
            }
            return Ok(Stmt {
                line,
                kind: StmtKind::Pass,
            });
        }

        // Augmented assignment (`+=`, `-=`, ...)
        if let Some(op) = aug_assign_op(self.peek_kind()) {
            self.bump();
            let value = self.parse_expr_or_tuple()?;
            let target = expr_to_target(first, &self.file)?;
            return Ok(Stmt {
                line,
                kind: StmtKind::AugAssign { target, op, value },
            });
        }

        // Plain or chained assignment (`a = b = val`)
        if matches!(self.peek_kind(), TokenKind::Eq) {
            let mut targets = vec![expr_to_target(first, &self.file)?];
            let mut value = None;
            while matches!(self.peek_kind(), TokenKind::Eq) {
                self.bump();
                let rhs = self.parse_expr_or_tuple()?;
                if matches!(self.peek_kind(), TokenKind::Eq) {
                    targets.push(expr_to_target(rhs, &self.file)?);
                } else {
                    value = Some(rhs);
                }
            }
            return Ok(Stmt {
                line,
                kind: StmtKind::Assign {
                    targets,
                    value: value.expect("rhs present"),
                },
            });
        }

        Ok(Stmt {
            line,
            kind: StmtKind::Expr(first),
        })
    }

    fn parse_target_list(&mut self) -> Result<Target, PyError> {
        let mut items = Vec::new();
        let mut had_comma = false;
        loop {
            let expr = self.parse_target_atom()?;
            items.push(expr_to_target(expr, &self.file)?);
            if matches!(self.peek_kind(), TokenKind::Comma) {
                had_comma = true;
                self.bump();
                if matches!(self.peek_kind(), TokenKind::In) {
                    break;
                }
            } else {
                break;
            }
        }
        if had_comma || items.len() > 1 {
            Ok(Target::Tuple(items))
        } else {
            Ok(items.remove(0))
        }
    }

    fn parse_target_atom(&mut self) -> Result<Expr, PyError> {
        if matches!(self.peek_kind(), TokenKind::Star) {
            let line = self.peek().line;
            self.bump();
            let inner = self.parse_postfix()?;
            return Ok(Expr {
                line,
                kind: ExprKind::Starred(Box::new(inner)),
            });
        }
        self.parse_postfix()
    }

    fn parse_expr_or_tuple(&mut self) -> Result<Expr, PyError> {
        let line = self.peek().line;
        let first = self.parse_star_expr()?;
        if !matches!(self.peek_kind(), TokenKind::Comma) {
            return Ok(first);
        }
        let mut elts = vec![first];
        while matches!(self.peek_kind(), TokenKind::Comma) {
            self.bump();
            if matches!(
                self.peek_kind(),
                TokenKind::Newline
                    | TokenKind::Semi
                    | TokenKind::Eof
                    | TokenKind::Eq
                    | TokenKind::Colon
                    | TokenKind::RParen
                    | TokenKind::RBracket
                    | TokenKind::RBrace
                    | TokenKind::Dedent
            ) {
                break;
            }
            elts.push(self.parse_star_expr()?);
        }
        Ok(Expr {
            line,
            kind: ExprKind::Tuple(elts),
        })
    }

    fn parse_star_expr(&mut self) -> Result<Expr, PyError> {
        if matches!(self.peek_kind(), TokenKind::Star) {
            let line = self.peek().line;
            self.bump();
            let inner = self.parse_expr()?;
            Ok(Expr {
                line,
                kind: ExprKind::Starred(Box::new(inner)),
            })
        } else {
            self.parse_expr()
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, PyError> {
        // Walrus operator `name := expr`
        if let TokenKind::Ident(name) = self.peek_kind().clone()
            && self.peek_kind_at(1) == Some(&TokenKind::ColonEq)
        {
            let line = self.peek().line;
            self.bump();
            self.bump();
            let val = self.parse_expr()?;
            return Ok(Expr {
                line,
                kind: ExprKind::Named {
                    target: name,
                    value: Box::new(val),
                },
            });
        }

        if matches!(self.peek_kind(), TokenKind::Lambda) {
            return self.parse_lambda();
        }

        let body = self.parse_or()?;
        if matches!(self.peek_kind(), TokenKind::If) {
            let line = body.line;
            self.bump();
            let test = self.parse_or()?;
            self.expect(&TokenKind::Else)?;
            let orelse = self.parse_expr()?;
            return Ok(Expr {
                line,
                kind: ExprKind::IfExp {
                    test: Box::new(test),
                    body: Box::new(body),
                    orelse: Box::new(orelse),
                },
            });
        }
        Ok(body)
    }

    fn parse_lambda(&mut self) -> Result<Expr, PyError> {
        let line = self.peek().line;
        self.expect(&TokenKind::Lambda)?;
        let params = if matches!(self.peek_kind(), TokenKind::Colon) {
            Params::default()
        } else {
            self.parse_params(TokenKind::Colon)?
        };
        self.expect(&TokenKind::Colon)?;
        let body = self.parse_expr()?;
        Ok(Expr {
            line,
            kind: ExprKind::Lambda {
                params,
                body: Box::new(body),
            },
        })
    }

    fn parse_or(&mut self) -> Result<Expr, PyError> {
        let first = self.parse_and()?;
        if !matches!(self.peek_kind(), TokenKind::Or) {
            return Ok(first);
        }
        let line = first.line;
        let mut values = vec![first];
        while matches!(self.peek_kind(), TokenKind::Or) {
            self.bump();
            values.push(self.parse_and()?);
        }
        Ok(Expr {
            line,
            kind: ExprKind::BoolOp {
                op: BoolOp::Or,
                values,
            },
        })
    }

    fn parse_and(&mut self) -> Result<Expr, PyError> {
        let first = self.parse_not()?;
        if !matches!(self.peek_kind(), TokenKind::And) {
            return Ok(first);
        }
        let line = first.line;
        let mut values = vec![first];
        while matches!(self.peek_kind(), TokenKind::And) {
            self.bump();
            values.push(self.parse_not()?);
        }
        Ok(Expr {
            line,
            kind: ExprKind::BoolOp {
                op: BoolOp::And,
                values,
            },
        })
    }

    fn parse_not(&mut self) -> Result<Expr, PyError> {
        if matches!(self.peek_kind(), TokenKind::Not) {
            let line = self.peek().line;
            self.bump();
            let operand = self.parse_not()?;
            Ok(Expr {
                line,
                kind: ExprKind::Unary {
                    op: UnaryOp::Not,
                    operand: Box::new(operand),
                },
            })
        } else {
            self.parse_compare()
        }
    }

    fn parse_compare(&mut self) -> Result<Expr, PyError> {
        let left = self.parse_bitor()?;
        let mut ops = Vec::new();
        let mut comparators = Vec::new();
        loop {
            let op = match self.peek_kind() {
                TokenKind::EqEq => {
                    self.bump();
                    CmpOp::Eq
                }
                TokenKind::NotEq => {
                    self.bump();
                    CmpOp::NotEq
                }
                TokenKind::Lt => {
                    self.bump();
                    CmpOp::Lt
                }
                TokenKind::LtEq => {
                    self.bump();
                    CmpOp::LtE
                }
                TokenKind::Gt => {
                    self.bump();
                    CmpOp::Gt
                }
                TokenKind::GtEq => {
                    self.bump();
                    CmpOp::GtE
                }
                TokenKind::In => {
                    self.bump();
                    CmpOp::In
                }
                TokenKind::Not if self.peek_kind_at(1) == Some(&TokenKind::In) => {
                    self.bump();
                    self.bump();
                    CmpOp::NotIn
                }
                TokenKind::Is => {
                    self.bump();
                    if matches!(self.peek_kind(), TokenKind::Not) {
                        self.bump();
                        CmpOp::IsNot
                    } else {
                        CmpOp::Is
                    }
                }
                _ => break,
            };
            ops.push(op);
            comparators.push(self.parse_bitor()?);
        }
        if ops.is_empty() {
            Ok(left)
        } else {
            let line = left.line;
            Ok(Expr {
                line,
                kind: ExprKind::Compare {
                    left: Box::new(left),
                    ops,
                    comparators,
                },
            })
        }
    }

    fn parse_bitor(&mut self) -> Result<Expr, PyError> {
        let mut left = self.parse_bitxor()?;
        while matches!(self.peek_kind(), TokenKind::Pipe) {
            let line = left.line;
            self.bump();
            let right = self.parse_bitxor()?;
            left = Expr {
                line,
                kind: ExprKind::Binary {
                    left: Box::new(left),
                    op: BinOp::BitOr,
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_bitxor(&mut self) -> Result<Expr, PyError> {
        let mut left = self.parse_bitand()?;
        while matches!(self.peek_kind(), TokenKind::Caret) {
            let line = left.line;
            self.bump();
            let right = self.parse_bitand()?;
            left = Expr {
                line,
                kind: ExprKind::Binary {
                    left: Box::new(left),
                    op: BinOp::BitXor,
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_bitand(&mut self) -> Result<Expr, PyError> {
        let mut left = self.parse_shift()?;
        while matches!(self.peek_kind(), TokenKind::Amp) {
            let line = left.line;
            self.bump();
            let right = self.parse_shift()?;
            left = Expr {
                line,
                kind: ExprKind::Binary {
                    left: Box::new(left),
                    op: BinOp::BitAnd,
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<Expr, PyError> {
        let mut left = self.parse_add()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::LShift => BinOp::LShift,
                TokenKind::RShift => BinOp::RShift,
                _ => break,
            };
            let line = left.line;
            self.bump();
            let right = self.parse_add()?;
            left = Expr {
                line,
                kind: ExprKind::Binary {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> Result<Expr, PyError> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            let line = left.line;
            self.bump();
            let right = self.parse_mul()?;
            left = Expr {
                line,
                kind: ExprKind::Binary {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, PyError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::DoubleSlash => BinOp::FloorDiv,
                TokenKind::Percent => BinOp::Mod,
                _ => break,
            };
            let line = left.line;
            self.bump();
            let right = self.parse_unary()?;
            left = Expr {
                line,
                kind: ExprKind::Binary {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, PyError> {
        let line = self.peek().line;
        match self.peek_kind() {
            TokenKind::Plus => {
                self.bump();
                let operand = self.parse_unary()?;
                Ok(Expr {
                    line,
                    kind: ExprKind::Unary {
                        op: UnaryOp::Pos,
                        operand: Box::new(operand),
                    },
                })
            }
            TokenKind::Minus => {
                self.bump();
                let operand = self.parse_unary()?;
                Ok(Expr {
                    line,
                    kind: ExprKind::Unary {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                })
            }
            TokenKind::Tilde => {
                self.bump();
                let operand = self.parse_unary()?;
                Ok(Expr {
                    line,
                    kind: ExprKind::Unary {
                        op: UnaryOp::BitNot,
                        operand: Box::new(operand),
                    },
                })
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> Result<Expr, PyError> {
        let base = self.parse_postfix()?;
        if matches!(self.peek_kind(), TokenKind::DoubleStar) {
            let line = base.line;
            self.bump();
            let exp = self.parse_unary()?;
            Ok(Expr {
                line,
                kind: ExprKind::Binary {
                    left: Box::new(base),
                    op: BinOp::Pow,
                    right: Box::new(exp),
                },
            })
        } else {
            Ok(base)
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, PyError> {
        let mut expr = self.parse_primary()?;
        loop {
            let line = self.peek().line;
            match self.peek_kind() {
                TokenKind::Dot => {
                    self.bump();
                    let attr = self.expect_ident()?;
                    expr = Expr {
                        line,
                        kind: ExprKind::Attribute {
                            value: Box::new(expr),
                            attr,
                        },
                    };
                }
                TokenKind::LParen => {
                    self.bump();
                    let args = self.parse_call_args()?;
                    self.expect(&TokenKind::RParen)?;
                    expr = Expr {
                        line,
                        kind: ExprKind::Call {
                            func: Box::new(expr),
                            args,
                        },
                    };
                }
                TokenKind::LBracket => {
                    self.bump();
                    let slice = self.parse_subscript_slice()?;
                    self.expect(&TokenKind::RBracket)?;
                    expr = Expr {
                        line,
                        kind: ExprKind::Subscript {
                            value: Box::new(expr),
                            slice: Box::new(slice),
                        },
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_call_args(&mut self) -> Result<Vec<CallArg>, PyError> {
        let mut args = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
            if matches!(self.peek_kind(), TokenKind::DoubleStar) {
                self.bump();
                args.push(CallArg::KwStarred(self.parse_expr()?));
            } else if matches!(self.peek_kind(), TokenKind::Star) {
                self.bump();
                args.push(CallArg::Starred(self.parse_expr()?));
            } else if let TokenKind::Ident(k) = self.peek_kind().clone()
                && self.peek_kind_at(1) == Some(&TokenKind::Eq)
            {
                self.bump();
                self.bump();
                args.push(CallArg::Keyword(k, self.parse_expr()?));
            } else {
                let first = self.parse_expr()?;
                // Generator expression as sole argument: `sum(x for x in xs)`
                if matches!(self.peek_kind(), TokenKind::For) {
                    let line = first.line;
                    let generators = self.parse_comprehensions()?;
                    args.push(CallArg::Positional(Expr {
                        line,
                        kind: ExprKind::ListComp {
                            elt: Box::new(first),
                            generators,
                        },
                    }));
                } else {
                    args.push(CallArg::Positional(first));
                }
            }
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
            } else {
                break;
            }
        }
        Ok(args)
    }

    fn parse_subscript_slice(&mut self) -> Result<SlicePart, PyError> {
        let mut lower: Option<Expr> = None;
        if !matches!(self.peek_kind(), TokenKind::Colon) {
            let first = self.parse_expr()?;
            if matches!(self.peek_kind(), TokenKind::Comma) {
                // Tuple index `a[0, 1]`
                let line = first.line;
                let mut elts = vec![first];
                while matches!(self.peek_kind(), TokenKind::Comma) {
                    self.bump();
                    if matches!(self.peek_kind(), TokenKind::RBracket) {
                        break;
                    }
                    elts.push(self.parse_expr()?);
                }
                return Ok(SlicePart::Index(Expr {
                    line,
                    kind: ExprKind::Tuple(elts),
                }));
            }
            if !matches!(self.peek_kind(), TokenKind::Colon) {
                return Ok(SlicePart::Index(first));
            }
            lower = Some(first);
        }
        // We saw `:` -> slice
        self.expect(&TokenKind::Colon)?;
        let upper = if matches!(self.peek_kind(), TokenKind::Colon | TokenKind::RBracket) {
            None
        } else {
            Some(Box::new(self.parse_expr()?))
        };
        let step = if matches!(self.peek_kind(), TokenKind::Colon) {
            self.bump();
            if matches!(self.peek_kind(), TokenKind::RBracket) {
                None
            } else {
                Some(Box::new(self.parse_expr()?))
            }
        } else {
            None
        };
        Ok(SlicePart::Slice {
            lower: lower.map(Box::new),
            upper,
            step,
        })
    }

    fn parse_primary(&mut self) -> Result<Expr, PyError> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::None => {
                self.bump();
                Ok(Expr {
                    line: tok.line,
                    kind: ExprKind::None,
                })
            }
            TokenKind::True => {
                self.bump();
                Ok(Expr {
                    line: tok.line,
                    kind: ExprKind::Bool(true),
                })
            }
            TokenKind::False => {
                self.bump();
                Ok(Expr {
                    line: tok.line,
                    kind: ExprKind::Bool(false),
                })
            }
            TokenKind::Int(n) => {
                self.bump();
                Ok(Expr {
                    line: tok.line,
                    kind: ExprKind::Int(n),
                })
            }
            TokenKind::Float(f) => {
                self.bump();
                Ok(Expr {
                    line: tok.line,
                    kind: ExprKind::Float(f),
                })
            }
            TokenKind::Ellipsis => {
                self.bump();
                Ok(Expr {
                    line: tok.line,
                    kind: ExprKind::None,
                })
            }
            TokenKind::Str(_) | TokenKind::FStr { .. } => self.parse_string_concat(),
            TokenKind::Ident(name) => {
                self.bump();
                Ok(Expr {
                    line: tok.line,
                    kind: ExprKind::Name(name),
                })
            }
            TokenKind::LParen => self.parse_paren_expr(),
            TokenKind::LBracket => self.parse_list_or_comp(),
            TokenKind::LBrace => self.parse_dict_or_set(),
            other => Err(self.err(format!("unexpected token {other:?} in expression"))),
        }
    }

    fn parse_string_concat(&mut self) -> Result<Expr, PyError> {
        let line = self.peek().line;
        let mut parts: Vec<FStringPart> = Vec::new();
        let mut has_fstr = false;

        while matches!(self.peek_kind(), TokenKind::Str(_) | TokenKind::FStr { .. }) {
            let t = self.bump();
            match t.kind {
                TokenKind::Str(s) => {
                    if let Some(FStringPart::Literal(prev)) = parts.last_mut() {
                        prev.push_str(&s);
                    } else {
                        parts.push(FStringPart::Literal(s));
                    }
                }
                TokenKind::FStr { raw, content } => {
                    has_fstr = true;
                    let sub = parse_fstring_parts(&self.file, t.line, raw, &content)?;
                    parts.extend(sub);
                }
                _ => unreachable!(),
            }
        }

        if !has_fstr {
            let text = match parts.into_iter().next() {
                Some(FStringPart::Literal(s)) => s,
                _ => String::new(),
            };
            Ok(Expr {
                line,
                kind: ExprKind::Str(text),
            })
        } else {
            Ok(Expr {
                line,
                kind: ExprKind::FString(parts),
            })
        }
    }

    fn parse_paren_expr(&mut self) -> Result<Expr, PyError> {
        let line = self.peek().line;
        self.expect(&TokenKind::LParen)?;
        if matches!(self.peek_kind(), TokenKind::RParen) {
            self.bump();
            return Ok(Expr {
                line,
                kind: ExprKind::Tuple(Vec::new()),
            });
        }
        let first = self.parse_star_expr()?;
        if matches!(self.peek_kind(), TokenKind::For) {
            // Generator expression `(x for x in xs)` -> evaluate eagerly as list comprehension
            let generators = self.parse_comprehensions()?;
            self.expect(&TokenKind::RParen)?;
            return Ok(Expr {
                line,
                kind: ExprKind::ListComp {
                    elt: Box::new(first),
                    generators,
                },
            });
        }
        if matches!(self.peek_kind(), TokenKind::Comma) {
            let mut elts = vec![first];
            while matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
                if matches!(self.peek_kind(), TokenKind::RParen) {
                    break;
                }
                elts.push(self.parse_star_expr()?);
            }
            self.expect(&TokenKind::RParen)?;
            return Ok(Expr {
                line,
                kind: ExprKind::Tuple(elts),
            });
        }
        self.expect(&TokenKind::RParen)?;
        Ok(first)
    }

    fn parse_list_or_comp(&mut self) -> Result<Expr, PyError> {
        let line = self.peek().line;
        self.expect(&TokenKind::LBracket)?;
        if matches!(self.peek_kind(), TokenKind::RBracket) {
            self.bump();
            return Ok(Expr {
                line,
                kind: ExprKind::List(Vec::new()),
            });
        }
        let first = self.parse_star_expr()?;
        if matches!(self.peek_kind(), TokenKind::For) {
            let generators = self.parse_comprehensions()?;
            self.expect(&TokenKind::RBracket)?;
            return Ok(Expr {
                line,
                kind: ExprKind::ListComp {
                    elt: Box::new(first),
                    generators,
                },
            });
        }
        let mut elts = vec![first];
        while matches!(self.peek_kind(), TokenKind::Comma) {
            self.bump();
            if matches!(self.peek_kind(), TokenKind::RBracket) {
                break;
            }
            elts.push(self.parse_star_expr()?);
        }
        self.expect(&TokenKind::RBracket)?;
        Ok(Expr {
            line,
            kind: ExprKind::List(elts),
        })
    }

    fn parse_dict_or_set(&mut self) -> Result<Expr, PyError> {
        let line = self.peek().line;
        self.expect(&TokenKind::LBrace)?;
        if matches!(self.peek_kind(), TokenKind::RBrace) {
            self.bump();
            return Ok(Expr {
                line,
                kind: ExprKind::Dict(Vec::new()),
            });
        }
        let first = self.parse_expr()?;
        if matches!(self.peek_kind(), TokenKind::Colon) {
            self.bump();
            let first_val = self.parse_expr()?;
            if matches!(self.peek_kind(), TokenKind::For) {
                let generators = self.parse_comprehensions()?;
                self.expect(&TokenKind::RBrace)?;
                return Ok(Expr {
                    line,
                    kind: ExprKind::DictComp {
                        key: Box::new(first),
                        value: Box::new(first_val),
                        generators,
                    },
                });
            }
            let mut entries = vec![(first, first_val)];
            while matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
                if matches!(self.peek_kind(), TokenKind::RBrace) {
                    break;
                }
                let k = self.parse_expr()?;
                self.expect(&TokenKind::Colon)?;
                let v = self.parse_expr()?;
                entries.push((k, v));
            }
            self.expect(&TokenKind::RBrace)?;
            return Ok(Expr {
                line,
                kind: ExprKind::Dict(entries),
            });
        }

        // Set comprehension or set literal
        if matches!(self.peek_kind(), TokenKind::For) {
            let generators = self.parse_comprehensions()?;
            self.expect(&TokenKind::RBrace)?;
            return Ok(Expr {
                line,
                kind: ExprKind::SetComp {
                    elt: Box::new(first),
                    generators,
                },
            });
        }
        let mut elts = vec![first];
        while matches!(self.peek_kind(), TokenKind::Comma) {
            self.bump();
            if matches!(self.peek_kind(), TokenKind::RBrace) {
                break;
            }
            elts.push(self.parse_expr()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Expr {
            line,
            kind: ExprKind::Set(elts),
        })
    }

    fn parse_comprehensions(&mut self) -> Result<Vec<Comprehension>, PyError> {
        let mut gens = Vec::new();
        while matches!(self.peek_kind(), TokenKind::For) {
            self.bump();
            let target = self.parse_target_list()?;
            self.expect(&TokenKind::In)?;
            let iter = self.parse_or()?;
            let mut ifs = Vec::new();
            while matches!(self.peek_kind(), TokenKind::If) {
                self.bump();
                ifs.push(self.parse_or()?);
            }
            gens.push(Comprehension { target, iter, ifs });
        }
        Ok(gens)
    }
}

fn aug_assign_op(kind: &TokenKind) -> Option<BinOp> {
    match kind {
        TokenKind::PlusEq => Some(BinOp::Add),
        TokenKind::MinusEq => Some(BinOp::Sub),
        TokenKind::StarEq => Some(BinOp::Mul),
        TokenKind::SlashEq => Some(BinOp::Div),
        TokenKind::DoubleSlashEq => Some(BinOp::FloorDiv),
        TokenKind::PercentEq => Some(BinOp::Mod),
        TokenKind::DoubleStarEq => Some(BinOp::Pow),
        TokenKind::AmpEq => Some(BinOp::BitAnd),
        TokenKind::PipeEq => Some(BinOp::BitOr),
        TokenKind::CaretEq => Some(BinOp::BitXor),
        TokenKind::LShiftEq => Some(BinOp::LShift),
        TokenKind::RShiftEq => Some(BinOp::RShift),
        _ => None,
    }
}

fn expr_to_target(expr: Expr, file: &str) -> Result<Target, PyError> {
    let line = expr.line;
    match expr.kind {
        ExprKind::Name(s) => Ok(Target::Name(s)),
        ExprKind::Attribute { value, attr } => Ok(Target::Attr { value, attr }),
        ExprKind::Subscript { value, slice } => match *slice {
            SlicePart::Index(index) => Ok(Target::Subscript {
                value,
                index: Box::new(index),
            }),
            SlicePart::Slice { .. } => Err(PyError::syntax(
                file,
                line,
                1,
                "slice assignment is not supported",
            )),
        },
        ExprKind::Tuple(elts) | ExprKind::List(elts) => {
            let mut targets = Vec::with_capacity(elts.len());
            for e in elts {
                targets.push(expr_to_target(e, file)?);
            }
            Ok(Target::Tuple(targets))
        }
        ExprKind::Starred(inner) => Ok(Target::Starred(Box::new(expr_to_target(*inner, file)?))),
        _ => Err(PyError::syntax(file, line, 1, "cannot assign to expression")),
    }
}

fn parse_fstring_parts(
    file: &str,
    line: usize,
    raw: bool,
    content: &str,
) -> Result<Vec<FStringPart>, PyError> {
    let chars: Vec<char> = content.chars().collect();
    let mut parts = Vec::new();
    let mut lit = String::new();
    let mut i = 0usize;

    let flush_lit = |lit: &mut String, parts: &mut Vec<FStringPart>| {
        if !lit.is_empty() {
            let s = std::mem::take(lit);
            let decoded = if raw { s } else { unescape_python_str(&s) };
            parts.push(FStringPart::Literal(decoded));
        }
    };

    while i < chars.len() {
        let ch = chars[i];
        if ch == '\\' && i + 1 < chars.len() {
            lit.push(ch);
            lit.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if ch == '{' {
            if chars.get(i + 1) == Some(&'{') {
                lit.push('{');
                i += 2;
                continue;
            }
            flush_lit(&mut lit, &mut parts);
            i += 1;
            let start = i;
            let mut depth = 1usize;
            let mut in_quote: Option<char> = None;
            let mut colon_idx: Option<usize> = None;
            while i < chars.len() && depth > 0 {
                let c = chars[i];
                if let Some(q) = in_quote {
                    if c == '\\' {
                        i += 2;
                        continue;
                    }
                    if c == q {
                        in_quote = None;
                    }
                    i += 1;
                    continue;
                }
                match c {
                    '\'' | '"' => in_quote = Some(c),
                    '{' | '(' | '[' => depth += 1,
                    '}' | ')' | ']' => {
                        depth -= 1;
                        if depth == 0 {
                            if c != '}' {
                                return Err(PyError::syntax(
                                    file,
                                    line,
                                    1,
                                    "mismatched delimiter in f-string",
                                ));
                            }
                            break;
                        }
                    }
                    ':' if depth == 1 && colon_idx.is_none() => {
                        colon_idx = Some(i);
                    }
                    _ => {}
                }
                i += 1;
            }
            if depth != 0 {
                return Err(PyError::syntax(file, line, 1, "unclosed '{' in f-string"));
            }
            let end = i;
            i += 1; // consume '}'

            let (expr_slice, format_spec) = if let Some(ci) = colon_idx {
                let e: String = chars[start..ci].iter().collect();
                let spec: String = chars[ci + 1..end].iter().collect();
                (e, Some(spec))
            } else {
                let e: String = chars[start..end].iter().collect();
                (e, None)
            };

            let (expr_clean, conversion) =
                if let Some(stripped) = expr_slice.trim_end().strip_suffix("!r") {
                    (stripped.trim(), Some('r'))
                } else if let Some(stripped) = expr_slice.trim_end().strip_suffix("!s") {
                    (stripped.trim(), Some('s'))
                } else {
                    (expr_slice.trim(), None)
                };

            if expr_clean.is_empty() {
                return Err(PyError::syntax(
                    file,
                    line,
                    1,
                    "empty expression in f-string",
                ));
            }
            let expr = parse_expr_str(file, line, expr_clean)?;
            parts.push(FStringPart::Formatted {
                expr,
                format_spec,
                conversion,
            });
            continue;
        }
        if ch == '}' {
            if chars.get(i + 1) == Some(&'}') {
                lit.push('}');
                i += 2;
                continue;
            }
            return Err(PyError::syntax(
                file,
                line,
                1,
                "single '}' is not allowed in f-string",
            ));
        }
        lit.push(ch);
        i += 1;
    }
    flush_lit(&mut lit, &mut parts);
    Ok(parts)
}
