//! Tokenizer. Newlines are only significant at bracket depth zero — they end
//! a statement — so a long call may wrap across lines inside its brackets.
//! `//` and `#` start line comments outside strings, and Markdown fence lines
//! (```` ``` ````) are ignored so a fenced spec still parses.

use crate::error::Issue;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    Ident(String),
    /// `$name` — reactive state, always rejected by the parser.
    StateVar(String),
    /// `@name` — runtime builtin, always rejected by the parser.
    Builtin(String),
    Str(String),
    Num(f64),
    True,
    False,
    Null,
    LParen,
    RParen,
    LBrack,
    RBrack,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Equals,
    /// Any operator (`+`, `==`, `.`, `?`, …). UI blocks hold literal data
    /// only, so the parser turns these into errors with a precise location.
    Op(&'static str),
    Newline,
    Unknown(char),
    Eof,
}

impl Tok {
    pub(crate) fn describe(&self) -> String {
        match self {
            Tok::Ident(name) => format!("`{name}`"),
            Tok::StateVar(name) => format!("`{name}`"),
            Tok::Builtin(name) => format!("`@{name}`"),
            Tok::Str(_) => "a string".into(),
            Tok::Num(_) => "a number".into(),
            Tok::True => "`true`".into(),
            Tok::False => "`false`".into(),
            Tok::Null => "`null`".into(),
            Tok::LParen => "`(`".into(),
            Tok::RParen => "`)`".into(),
            Tok::LBrack => "`[`".into(),
            Tok::RBrack => "`]`".into(),
            Tok::LBrace => "`{`".into(),
            Tok::RBrace => "`}`".into(),
            Tok::Comma => "`,`".into(),
            Tok::Colon => "`:`".into(),
            Tok::Equals => "`=`".into(),
            Tok::Op(op) => format!("`{op}`"),
            Tok::Newline => "the end of the line".into(),
            Tok::Unknown(c) => format!("`{c}`"),
            Tok::Eof => "the end of the spec".into(),
        }
    }

    /// Tokens after which a `-` is subtraction rather than a negative sign.
    fn ends_value(&self) -> bool {
        matches!(
            self,
            Tok::Num(_)
                | Tok::Str(_)
                | Tok::Ident(_)
                | Tok::RParen
                | Tok::RBrack
                | Tok::True
                | Tok::False
                | Tok::Null
                | Tok::StateVar(_)
                | Tok::Builtin(_)
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub tok: Tok,
    pub line: usize,
}

pub(crate) fn tokenize(src: &str) -> Result<Vec<Token>, Vec<Issue>> {
    Lexer {
        chars: src.chars().collect(),
        pos: 0,
        line: 1,
        depth: 0,
        tokens: Vec::new(),
        issues: Vec::new(),
    }
    .run()
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    depth: usize,
    tokens: Vec<Token>,
    issues: Vec<Issue>,
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

impl Lexer {
    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn push(&mut self, tok: Tok, line: usize) {
        self.tokens.push(Token { tok, line });
    }

    fn at_line_start(&self) -> bool {
        self.chars[..self.pos]
            .iter()
            .rev()
            .take_while(|c| **c != '\n')
            .all(|c| c.is_whitespace())
    }

    fn skip_to_newline(&mut self) {
        while let Some(c) = self.peek(0) {
            if c == '\n' {
                break;
            }
            self.pos += 1;
        }
    }

    fn run(mut self) -> Result<Vec<Token>, Vec<Issue>> {
        while let Some(c) = self.peek(0) {
            let line = self.line;
            match c {
                ' ' | '\t' | '\r' => self.pos += 1,
                '\n' => {
                    if self.depth == 0 {
                        self.push(Tok::Newline, line);
                    }
                    self.line += 1;
                    self.pos += 1;
                }
                '`' if self.peek(1) == Some('`')
                    && self.peek(2) == Some('`')
                    && self.at_line_start() =>
                {
                    self.skip_to_newline()
                }
                '#' => self.skip_to_newline(),
                '/' if self.peek(1) == Some('/') => self.skip_to_newline(),
                '(' | '[' | '{' => {
                    self.depth += 1;
                    self.pos += 1;
                    let tok = match c {
                        '(' => Tok::LParen,
                        '[' => Tok::LBrack,
                        _ => Tok::LBrace,
                    };
                    self.push(tok, line);
                }
                ')' | ']' | '}' => {
                    self.depth = self.depth.saturating_sub(1);
                    self.pos += 1;
                    let tok = match c {
                        ')' => Tok::RParen,
                        ']' => Tok::RBrack,
                        _ => Tok::RBrace,
                    };
                    self.push(tok, line);
                }
                ',' => {
                    self.pos += 1;
                    self.push(Tok::Comma, line);
                }
                ':' => {
                    self.pos += 1;
                    self.push(Tok::Colon, line);
                }
                '"' => self.double_quoted(),
                '\'' => self.single_quoted(),
                '0'..='9' => self.number(),
                '-' if self.peek(1).is_some_and(|d| d.is_ascii_digit())
                    && !self.tokens.last().is_some_and(|t| t.tok.ends_value()) =>
                {
                    self.number()
                }
                '$' if self.peek(1).is_some_and(is_ident_start) => {
                    self.pos += 1;
                    let name = self.word();
                    self.push(Tok::StateVar(format!("${name}")), line);
                }
                '@' if self.peek(1).is_some_and(is_ident_start) => {
                    self.pos += 1;
                    let name = self.word();
                    self.push(Tok::Builtin(name), line);
                }
                c if is_ident_start(c) => {
                    let word = self.word();
                    let tok = match word.as_str() {
                        "true" => Tok::True,
                        "false" => Tok::False,
                        "null" => Tok::Null,
                        _ => Tok::Ident(word),
                    };
                    self.push(tok, line);
                }
                _ => self.operator(c, line),
            }
        }
        let line = self.line;
        self.push(Tok::Eof, line);
        if self.issues.is_empty() {
            Ok(self.tokens)
        } else {
            Err(self.issues)
        }
    }

    fn word(&mut self) -> String {
        let start = self.pos;
        while self.peek(0).is_some_and(is_ident_char) {
            self.pos += 1;
        }
        self.chars[start..self.pos].iter().collect()
    }

    fn operator(&mut self, c: char, line: usize) {
        let next = self.peek(1);
        let (op, len): (&'static str, usize) = match (c, next) {
            ('=', Some('=')) => ("==", 2),
            ('=', _) => ("=", 1),
            ('!', Some('=')) => ("!=", 2),
            ('!', _) => ("!", 1),
            ('>', Some('=')) => (">=", 2),
            ('>', _) => (">", 1),
            ('<', Some('=')) => ("<=", 2),
            ('<', _) => ("<", 1),
            ('&', Some('&')) => ("&&", 2),
            ('&', _) => ("&", 1),
            ('|', Some('|')) => ("||", 2),
            ('|', _) => ("|", 1),
            ('.', _) => (".", 1),
            ('?', _) => ("?", 1),
            ('+', _) => ("+", 1),
            ('-', _) => ("-", 1),
            ('*', _) => ("*", 1),
            ('/', _) => ("/", 1),
            ('%', _) => ("%", 1),
            _ => {
                self.pos += 1;
                self.push(Tok::Unknown(c), line);
                return;
            }
        };
        self.pos += len;
        let tok = if op == "=" { Tok::Equals } else { Tok::Op(op) };
        self.push(tok, line);
    }

    fn number(&mut self) {
        let line = self.line;
        let start = self.pos;
        if self.peek(0) == Some('-') {
            self.pos += 1;
        }
        let digits = |lexer: &mut Self| {
            while lexer.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                lexer.pos += 1;
            }
        };
        digits(self);
        if self.peek(0) == Some('.') && self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
            digits(self);
        }
        if matches!(self.peek(0), Some('e' | 'E')) {
            self.pos += 1;
            if matches!(self.peek(0), Some('+' | '-')) {
                self.pos += 1;
            }
            digits(self);
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        match text.parse::<f64>() {
            Ok(value) => self.push(Tok::Num(value), line),
            Err(_) => self.issues.push(Issue::at(
                line,
                format!("`{text}` is not a valid number; write numbers like 42, -3 or 1.5."),
            )),
        }
    }

    fn unclosed(&mut self, line: usize, quote: char) {
        self.issues.push(Issue::at(
            line,
            format!(
                "string is never closed — the spec looks truncated; end it with {quote} and send the complete spec."
            ),
        ));
    }

    /// `"…"` with JSON escapes. Raw line breaks inside are kept verbatim.
    fn double_quoted(&mut self) {
        let line = self.line;
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek(0) else {
                self.unclosed(line, '"');
                return;
            };
            self.pos += 1;
            match c {
                '"' => break,
                '\n' => {
                    self.line += 1;
                    out.push(c);
                }
                '\\' => {
                    let Some(esc) = self.peek(0) else {
                        self.unclosed(line, '"');
                        return;
                    };
                    self.pos += 1;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => match self.unicode_escape() {
                            Some(ch) => out.push(ch),
                            None => {
                                self.issues.push(Issue::at(
                                    self.line,
                                    "invalid `\\u` escape in string; use four hex digits like \\u00e9.",
                                ));
                                out.push('\u{fffd}');
                            }
                        },
                        other => {
                            self.issues.push(Issue::at(
                                self.line,
                                format!(
                                    "invalid escape `\\{other}` in string; use \\\" \\\\ \\n \\t or \\uXXXX."
                                ),
                            ));
                            if other == '\n' {
                                self.line += 1;
                            }
                        }
                    }
                }
                _ => out.push(c),
            }
        }
        self.push(Tok::Str(out), line);
    }

    fn hex4(&mut self) -> Option<u32> {
        let mut value = 0;
        for _ in 0..4 {
            let digit = self.peek(0)?.to_digit(16)?;
            self.pos += 1;
            value = value * 16 + digit;
        }
        Some(value)
    }

    fn unicode_escape(&mut self) -> Option<char> {
        let high = self.hex4()?;
        if (0xD800..0xDC00).contains(&high) {
            if self.peek(0) == Some('\\') && self.peek(1) == Some('u') {
                self.pos += 2;
                let low = self.hex4()?;
                if (0xDC00..0xE000).contains(&low) {
                    return char::from_u32(0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00));
                }
            }
            return None;
        }
        char::from_u32(high)
    }

    /// `'…'` with the small escape set the reference accepts.
    fn single_quoted(&mut self) {
        let line = self.line;
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek(0) else {
                self.unclosed(line, '\'');
                return;
            };
            self.pos += 1;
            match c {
                '\'' => break,
                '\n' => {
                    self.line += 1;
                    out.push(c);
                }
                '\\' => {
                    let Some(esc) = self.peek(0) else {
                        self.unclosed(line, '\'');
                        return;
                    };
                    self.pos += 1;
                    match esc {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        '\n' => {
                            self.line += 1;
                            out.push('\n');
                        }
                        other => out.push(other),
                    }
                }
                _ => out.push(c),
            }
        }
        self.push(Tok::Str(out), line);
    }
}
