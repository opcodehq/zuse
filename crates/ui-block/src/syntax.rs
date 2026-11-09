//! Statement parser: `name = Expr` per line, where an expression is a
//! literal, an array, an object, a component call, or a bare reference to
//! another statement. Anything dynamic (state, operators, builtins) is a
//! syntax error with a location, never silently dropped.

use crate::error::Issue;
use crate::lexer::{Tok, Token};

/// Literal nesting deeper than this can never pass the expansion bound, so
/// the parser stops early instead of recursing on hostile input.
pub(crate) const MAX_DEPTH: usize = 48;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Expr {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
    Arr(Vec<Node>),
    Obj(Vec<(String, Node)>),
    Ref(String),
    Call { name: String, args: Vec<Node> },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub expr: Expr,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Statement {
    pub name: String,
    pub line: usize,
    pub expr: Node,
}

pub(crate) fn parse_statements(tokens: &[Token]) -> Result<Vec<Statement>, Vec<Issue>> {
    let mut parser = Parser {
        tokens,
        pos: 0,
        open: Vec::new(),
    };
    let mut statements = Vec::new();
    let mut issues = Vec::new();
    loop {
        while parser.peek().tok == Tok::Newline {
            parser.pos += 1;
        }
        if parser.peek().tok == Tok::Eof {
            break;
        }
        match parser.statement() {
            Ok(statement) => statements.push(statement),
            Err(issue) => {
                issues.push(issue);
                parser.open.clear();
                while !matches!(parser.peek().tok, Tok::Newline | Tok::Eof) {
                    parser.pos += 1;
                }
            }
        }
    }
    if issues.is_empty() {
        Ok(statements)
    } else {
        Err(issues)
    }
}

const LITERALS_ONLY: &str = "UI blocks hold literal data only — write the final value directly.";

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// Lines of the brackets currently open, for truncation errors.
    open: Vec<(usize, char)>,
}

type Parsed<T> = Result<T, Issue>;

impl<'a> Parser<'a> {
    fn peek(&self) -> &'a Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn bump(&mut self) -> &'a Token {
        let token = self.peek();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        token
    }

    fn truncated(&self) -> Issue {
        match self.open.last() {
            Some(&(line, bracket)) => Issue::at(
                line,
                format!(
                    "`{bracket}` is never closed — the spec looks truncated. Close every bracket and send the complete spec."
                ),
            ),
            None => Issue::at(
                self.peek().line,
                "the spec ends in the middle of a statement — it looks truncated; send the complete spec.",
            ),
        }
    }

    fn unsupported(&self, token: &Token) -> Issue {
        let message = match &token.tok {
            Tok::StateVar(name) => format!(
                "`{name}` is reactive state, which UI blocks do not support. {LITERALS_ONLY}"
            ),
            Tok::Builtin(name) => format!(
                "`@{name}` is a runtime builtin, which UI blocks do not support. {LITERALS_ONLY}"
            ),
            Tok::Op(".") => format!("member access with `.` is not supported. {LITERALS_ONLY}"),
            Tok::Op("?") => {
                format!("conditional expressions (`? :`) are not supported. {LITERALS_ONLY}")
            }
            Tok::Op(op) => format!(
                "operators like `{op}` are not supported. {LITERALS_ONLY} Numbers may be negative (-3) or decimal (1.5)."
            ),
            Tok::Unknown(c) => format!("unexpected character `{c}`."),
            Tok::Eof => return self.truncated(),
            other => format!("unexpected {}.", other.describe()),
        };
        Issue::at(token.line, message)
    }

    fn statement(&mut self) -> Parsed<Statement> {
        let first = self.bump();
        let name = match &first.tok {
            Tok::Ident(name) => name.clone(),
            Tok::StateVar(name) => {
                return Err(Issue::at(
                    first.line,
                    format!(
                        "`{name}` is reactive state, which UI blocks do not support. {LITERALS_ONLY}"
                    ),
                ));
            }
            other => {
                return Err(Issue::at(
                    first.line,
                    format!(
                        "expected a statement like `name = Component(...)`, found {}. Put one statement per line and start with `root = ...`.",
                        other.describe()
                    ),
                ));
            }
        };
        let eq = self.bump();
        if eq.tok != Tok::Equals {
            return Err(Issue::at(
                eq.line,
                format!(
                    "expected `=` after `{name}`, found {}. Statements look like `{name} = Component(...)`.",
                    eq.tok.describe()
                ),
            ));
        }
        if matches!(self.peek().tok, Tok::Newline | Tok::Eof) {
            return Err(Issue::at(
                first.line,
                format!("`{name} =` has no value; write `{name} = Component(...)`."),
            ));
        }
        let expr = self.expr(0)?;
        let next = self.peek();
        match &next.tok {
            Tok::Newline | Tok::Eof => {}
            Tok::Op(_) | Tok::StateVar(_) | Tok::Builtin(_) | Tok::Unknown(_) => {
                return Err(self.unsupported(next));
            }
            other => {
                return Err(Issue::at(
                    next.line,
                    format!(
                        "unexpected {} after the value of `{name}`. Put each statement on its own line.",
                        other.describe()
                    ),
                ));
            }
        }
        Ok(Statement {
            name,
            line: first.line,
            expr,
        })
    }

    fn expr(&mut self, depth: usize) -> Parsed<Node> {
        if depth > MAX_DEPTH {
            return Err(Issue::at(
                self.peek().line,
                format!(
                    "UI spec is too complex: values nest more than {MAX_DEPTH} levels deep. Use fewer nested components or flatter data."
                ),
            ));
        }
        let token = self.peek();
        let line = token.line;
        let expr = match &token.tok {
            Tok::Str(value) => {
                self.bump();
                Expr::Str(value.clone())
            }
            Tok::Num(value) => {
                self.bump();
                Expr::Num(*value)
            }
            Tok::True => {
                self.bump();
                Expr::Bool(true)
            }
            Tok::False => {
                self.bump();
                Expr::Bool(false)
            }
            Tok::Null => {
                self.bump();
                Expr::Null
            }
            Tok::LBrack => Expr::Arr(self.list(depth, '[', Tok::RBrack)?),
            Tok::LBrace => self.object(depth)?,
            Tok::Ident(name) => {
                self.bump();
                if self.peek().tok == Tok::LParen {
                    let args = self.list(depth, '(', Tok::RParen)?;
                    Expr::Call {
                        name: name.clone(),
                        args,
                    }
                } else {
                    Expr::Ref(name.clone())
                }
            }
            Tok::LParen => {
                return Err(Issue::at(
                    line,
                    format!("grouping with parentheses is not supported. {LITERALS_ONLY}"),
                ));
            }
            _ => return Err(self.unsupported(token)),
        };
        // Anything operator-like directly after a value is an expression.
        let next = self.peek();
        if matches!(next.tok, Tok::Op(_)) {
            return Err(self.unsupported(next));
        }
        Ok(Node { expr, line })
    }

    /// `[a, b]` or `(a, b)`: comma-separated, trailing comma allowed.
    fn list(&mut self, depth: usize, bracket: char, close: Tok) -> Parsed<Vec<Node>> {
        let open = self.bump();
        self.open.push((open.line, bracket));
        let mut items = Vec::new();
        loop {
            if self.peek().tok == close {
                self.bump();
                break;
            }
            if self.peek().tok == Tok::Eof {
                return Err(self.truncated());
            }
            items.push(self.expr(depth + 1)?);
            let sep = self.peek();
            if sep.tok == Tok::Comma {
                self.bump();
            } else if sep.tok == close {
                continue;
            } else if sep.tok == Tok::Eof {
                return Err(self.truncated());
            } else {
                return Err(Issue::at(
                    sep.line,
                    format!(
                        "expected `,` or {} but found {}.",
                        close.describe(),
                        sep.tok.describe()
                    ),
                ));
            }
        }
        self.open.pop();
        Ok(items)
    }

    fn object(&mut self, depth: usize) -> Parsed<Expr> {
        let open = self.bump();
        self.open.push((open.line, '{'));
        let mut entries: Vec<(String, Node)> = Vec::new();
        loop {
            let key_token = self.bump();
            let key = match &key_token.tok {
                Tok::RBrace => break,
                Tok::Ident(key) | Tok::Str(key) => key.clone(),
                Tok::Eof => return Err(self.truncated()),
                other => {
                    return Err(Issue::at(
                        key_token.line,
                        format!(
                            "expected an object key like `label:` but found {}.",
                            other.describe()
                        ),
                    ));
                }
            };
            let colon = self.bump();
            if colon.tok != Tok::Colon {
                if colon.tok == Tok::Eof {
                    return Err(self.truncated());
                }
                return Err(Issue::at(
                    colon.line,
                    format!(
                        "expected `:` after object key `{key}` but found {}.",
                        colon.tok.describe()
                    ),
                ));
            }
            if entries.iter().any(|(existing, _)| *existing == key) {
                return Err(Issue::at(
                    key_token.line,
                    format!("object key `{key}` appears twice; keep one."),
                ));
            }
            let value = self.expr(depth + 1)?;
            entries.push((key, value));
            let sep = self.peek();
            match sep.tok {
                Tok::Comma => {
                    self.bump();
                }
                Tok::RBrace => {}
                Tok::Eof => return Err(self.truncated()),
                _ => {
                    return Err(Issue::at(
                        sep.line,
                        format!("expected `,` or `}}` but found {}.", sep.tok.describe()),
                    ));
                }
            }
        }
        self.open.pop();
        Ok(Expr::Obj(entries))
    }
}
