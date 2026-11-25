//! Indentation-aware Python 3 lexer (pure Rust, zero `unsafe`).

use crate::error::PyError;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Structure
    Indent,
    Dedent,
    Newline,
    Eof,

    // Literals & Names
    Int(i64),
    Float(f64),
    Str(String),
    FStr { raw: bool, content: String },
    Ident(String),

    // Keywords
    False,
    None,
    True,
    And,
    As,
    Assert,
    Break,
    Class,
    Continue,
    Def,
    Del,
    Elif,
    Else,
    Except,
    Finally,
    For,
    From,
    Global,
    If,
    Import,
    In,
    Is,
    Lambda,
    Nonlocal,
    Not,
    Or,
    Pass,
    Raise,
    Return,
    Try,
    While,

    // Operators & Delimiters
    Plus,
    Minus,
    Star,
    DoubleStar,
    Slash,
    DoubleSlash,
    Percent,
    At,
    Eq,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    PlusEq,
    MinusEq,
    StarEq,
    DoubleStarEq,
    SlashEq,
    DoubleSlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    LShiftEq,
    RShiftEq,
    ColonEq,
    Amp,
    Pipe,
    Caret,
    Tilde,
    LShift,
    RShift,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Dot,
    Semi,
    Arrow,
    Ellipsis,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub col: usize,
}

pub fn tokenize(file: &str, src: &str) -> Result<Vec<Token>, PyError> {
    let clean = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let chars: Vec<char> = clean.chars().collect();
    let mut lexer = Lexer {
        file: file.to_string(),
        chars,
        pos: 0,
        line: 1,
        col: 1,
        paren_depth: 0,
        indent_stack: vec![0],
        at_line_start: true,
        tokens: Vec::new(),
    };
    lexer.run()?;
    Ok(lexer.tokens)
}

struct Lexer {
    file: String,
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    paren_depth: usize,
    indent_stack: Vec<usize>,
    at_line_start: bool,
    tokens: Vec<Token>,
}

impl Lexer {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn err(&self, msg: impl Into<String>) -> PyError {
        PyError::syntax(&self.file, self.line, self.col, msg)
    }

    fn run(&mut self) -> Result<(), PyError> {
        while self.pos < self.chars.len() {
            if self.at_line_start && self.paren_depth == 0 {
                self.handle_indentation()?;
                if self.pos >= self.chars.len() {
                    break;
                }
            }

            let Some(ch) = self.peek() else {
                break;
            };

            match ch {
                ' ' | '\t' | '\x0C' => {
                    self.bump();
                }
                '\r' => {
                    self.bump();
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    self.emit_newline();
                }
                '\n' => {
                    self.bump();
                    self.emit_newline();
                }
                '\\' => {
                    self.bump();
                    if self.peek() == Some('\r') {
                        self.bump();
                    }
                    if self.peek() == Some('\n') {
                        self.bump();
                    } else {
                        return Err(self.err("unexpected character after line continuation '\\'"));
                    }
                }
                '#' => {
                    while let Some(c) = self.peek() {
                        if c == '\n' || c == '\r' {
                            break;
                        }
                        self.bump();
                    }
                }
                '\'' | '"' => {
                    let tok = self.lex_string(false, false)?;
                    self.tokens.push(tok);
                    self.at_line_start = false;
                }
                '0'..='9' => {
                    let tok = self.lex_number()?;
                    self.tokens.push(tok);
                    self.at_line_start = false;
                }
                '.' if self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) => {
                    let tok = self.lex_number()?;
                    self.tokens.push(tok);
                    self.at_line_start = false;
                }
                c if is_ident_start(c) => {
                    let tok = self.lex_ident_or_prefixed_str()?;
                    self.tokens.push(tok);
                    self.at_line_start = false;
                }
                _ => {
                    let tok = self.lex_operator()?;
                    self.tokens.push(tok);
                    self.at_line_start = false;
                }
            }
        }

        if self.paren_depth > 0 {
            return Err(self.err("unclosed parenthesis, bracket, or brace"));
        }

        if let Some(last) = self.tokens.last()
            && !matches!(
                last.kind,
                TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent
            )
        {
            self.tokens.push(Token {
                kind: TokenKind::Newline,
                line: self.line,
                col: self.col,
            });
        }

        while self.indent_stack.len() > 1 {
            self.indent_stack.pop();
            self.tokens.push(Token {
                kind: TokenKind::Dedent,
                line: self.line,
                col: self.col,
            });
        }

        self.tokens.push(Token {
            kind: TokenKind::Eof,
            line: self.line,
            col: self.col,
        });
        Ok(())
    }

    fn emit_newline(&mut self) {
        if self.paren_depth == 0 {
            if let Some(last) = self.tokens.last()
                && !matches!(
                    last.kind,
                    TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent
                )
            {
                self.tokens.push(Token {
                    kind: TokenKind::Newline,
                    line: self.line.saturating_sub(1).max(1),
                    col: 1,
                });
            }
            self.at_line_start = true;
        }
    }

    fn handle_indentation(&mut self) -> Result<(), PyError> {
        loop {
            let mut indent = 0usize;
            while let Some(c) = self.peek() {
                match c {
                    ' ' => {
                        indent += 1;
                        self.bump();
                    }
                    '\t' => {
                        indent = (indent / 8 + 1) * 8;
                        self.bump();
                    }
                    '\x0C' => {
                        self.bump();
                    }
                    _ => break,
                }
            }

            match self.peek() {
                None => {
                    self.at_line_start = false;
                    return Ok(());
                }
                Some('\n') => {
                    self.bump();
                    continue;
                }
                Some('\r') => {
                    self.bump();
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    continue;
                }
                Some('#') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' || c == '\r' {
                            break;
                        }
                        self.bump();
                    }
                    if self.peek() == Some('\r') {
                        self.bump();
                    }
                    if self.peek() == Some('\n') {
                        self.bump();
                    }
                    continue;
                }
                Some(_) => {
                    self.at_line_start = false;
                    let top = *self.indent_stack.last().unwrap_or(&0);
                    if indent > top {
                        self.indent_stack.push(indent);
                        self.tokens.push(Token {
                            kind: TokenKind::Indent,
                            line: self.line,
                            col: self.col,
                        });
                    } else if indent < top {
                        while let Some(&cur) = self.indent_stack.last() {
                            if cur > indent {
                                self.indent_stack.pop();
                                self.tokens.push(Token {
                                    kind: TokenKind::Dedent,
                                    line: self.line,
                                    col: self.col,
                                });
                            } else {
                                break;
                            }
                        }
                        if *self.indent_stack.last().unwrap_or(&0) != indent {
                            return Err(self.err("unindent does not match any outer indentation level"));
                        }
                    }
                    return Ok(());
                }
            }
        }
    }

    fn lex_ident_or_prefixed_str(&mut self) -> Result<Token, PyError> {
        let start_line = self.line;
        let start_col = self.col;

        // Check for string prefixes: f, r, u, b, fr, rf (case-insensitive)
        let saved_pos = self.pos;
        let saved_line = self.line;
        let saved_col = self.col;
        let mut prefix = String::new();
        while prefix.len() < 2
            && let Some(c) = self.peek()
            && matches!(c, 'f' | 'F' | 'r' | 'R' | 'u' | 'U' | 'b' | 'B')
        {
            prefix.push(c.to_ascii_lowercase());
            self.bump();
        }
        if !prefix.is_empty() && matches!(self.peek(), Some('\'' | '"')) {
            let is_f = prefix.contains('f');
            let is_r = prefix.contains('r');
            let valid = matches!(
                prefix.as_str(),
                "f" | "r" | "u" | "b" | "fr" | "rf" | "br" | "rb"
            );
            if valid {
                return self.lex_string(is_f, is_r);
            }
        }
        self.pos = saved_pos;
        self.line = saved_line;
        self.col = saved_col;

        let mut name = String::new();
        while let Some(c) = self.peek() {
            if is_ident_continue(c) {
                name.push(c);
                self.bump();
            } else {
                break;
            }
        }
        let kind = match name.as_str() {
            "False" => TokenKind::False,
            "None" => TokenKind::None,
            "True" => TokenKind::True,
            "and" => TokenKind::And,
            "as" => TokenKind::As,
            "assert" => TokenKind::Assert,
            "break" => TokenKind::Break,
            "class" => TokenKind::Class,
            "continue" => TokenKind::Continue,
            "def" => TokenKind::Def,
            "del" => TokenKind::Del,
            "elif" => TokenKind::Elif,
            "else" => TokenKind::Else,
            "except" => TokenKind::Except,
            "finally" => TokenKind::Finally,
            "for" => TokenKind::For,
            "from" => TokenKind::From,
            "global" => TokenKind::Global,
            "if" => TokenKind::If,
            "import" => TokenKind::Import,
            "in" => TokenKind::In,
            "is" => TokenKind::Is,
            "lambda" => TokenKind::Lambda,
            "nonlocal" => TokenKind::Nonlocal,
            "not" => TokenKind::Not,
            "or" => TokenKind::Or,
            "pass" => TokenKind::Pass,
            "raise" => TokenKind::Raise,
            "return" => TokenKind::Return,
            "try" => TokenKind::Try,
            "while" => TokenKind::While,
            _ => TokenKind::Ident(name),
        };
        Ok(Token {
            kind,
            line: start_line,
            col: start_col,
        })
    }

    fn lex_string(&mut self, is_fstr: bool, is_raw: bool) -> Result<Token, PyError> {
        let start_line = self.line;
        let start_col = self.col;
        let quote = self.bump().ok_or_else(|| self.err("expected string quote"))?;
        let triple = self.peek() == Some(quote) && self.peek_at(1) == Some(quote);
        if triple {
            self.bump();
            self.bump();
        }

        let mut raw_buf = String::new();
        let mut brace_depth = 0usize;
        let mut inner_quote: Option<char> = None;

        loop {
            let Some(ch) = self.peek() else {
                return Err(PyError::syntax(
                    &self.file,
                    start_line,
                    start_col,
                    "unterminated string literal",
                ));
            };

            if is_fstr && brace_depth > 0 {
                // Inside an f-string expression `{...}`: allow matching inner quotes
                if let Some(iq) = inner_quote {
                    raw_buf.push(ch);
                    self.bump();
                    if ch == '\\'
                        && let Some(next) = self.bump()
                    {
                        raw_buf.push(next);
                    } else if ch == iq {
                        inner_quote = None;
                    }
                    continue;
                }
                if ch == '\'' || ch == '"' {
                    // Only treat as inner quote if it is different from outer non-triple quote or inside triple
                    if triple || ch != quote {
                        inner_quote = Some(ch);
                        raw_buf.push(ch);
                        self.bump();
                        continue;
                    }
                }
                if ch == '{' {
                    brace_depth += 1;
                    raw_buf.push(ch);
                    self.bump();
                    continue;
                }
                if ch == '}' {
                    brace_depth -= 1;
                    raw_buf.push(ch);
                    self.bump();
                    continue;
                }
            }

            if ch == quote {
                if triple {
                    if self.peek_at(1) == Some(quote) && self.peek_at(2) == Some(quote) {
                        self.bump();
                        self.bump();
                        self.bump();
                        break;
                    }
                    raw_buf.push(ch);
                    self.bump();
                    continue;
                } else {
                    self.bump();
                    break;
                }
            }

            if !triple && (ch == '\n' || ch == '\r') {
                return Err(PyError::syntax(
                    &self.file,
                    start_line,
                    start_col,
                    "EOL while scanning string literal",
                ));
            }

            if is_fstr && brace_depth == 0 {
                if ch == '{' {
                    if self.peek_at(1) == Some('{') {
                        raw_buf.push('{');
                        raw_buf.push('{');
                        self.bump();
                        self.bump();
                        continue;
                    }
                    brace_depth += 1;
                    raw_buf.push(ch);
                    self.bump();
                    continue;
                }
                if ch == '}' && self.peek_at(1) == Some('}') {
                    raw_buf.push('}');
                    raw_buf.push('}');
                    self.bump();
                    self.bump();
                    continue;
                }
            }

            if ch == '\\' {
                raw_buf.push(ch);
                self.bump();
                if let Some(next) = self.bump() {
                    raw_buf.push(next);
                }
                continue;
            }

            raw_buf.push(ch);
            self.bump();
        }

        let kind = if is_fstr {
            TokenKind::FStr {
                raw: is_raw,
                content: raw_buf,
            }
        } else if is_raw {
            TokenKind::Str(raw_buf)
        } else {
            TokenKind::Str(unescape_python_str(&raw_buf))
        };
        Ok(Token {
            kind,
            line: start_line,
            col: start_col,
        })
    }

    fn lex_number(&mut self) -> Result<Token, PyError> {
        let start_line = self.line;
        let start_col = self.col;

        // Hex / Bin / Oct
        if self.peek() == Some('0')
            && let Some(rad_ch) = self.peek_at(1)
            && matches!(rad_ch, 'x' | 'X' | 'b' | 'B' | 'o' | 'O')
        {
            self.bump();
            self.bump();
            let radix = match rad_ch {
                'x' | 'X' => 16,
                'b' | 'B' => 2,
                _ => 8,
            };
            let mut digits = String::new();
            while let Some(c) = self.peek() {
                if c == '_' {
                    self.bump();
                } else if c.is_digit(radix) {
                    digits.push(c);
                    self.bump();
                } else {
                    break;
                }
            }
            let val = i64::from_str_radix(&digits, radix)
                .map_err(|_| PyError::syntax(&self.file, start_line, start_col, "invalid integer literal"))?;
            return Ok(Token {
                kind: TokenKind::Int(val),
                line: start_line,
                col: start_col,
            });
        }

        let mut buf = String::new();
        let mut is_float = false;

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                buf.push(c);
                self.bump();
            } else if c == '_' {
                self.bump();
            } else {
                break;
            }
        }

        if self.peek() == Some('.') && self.peek_at(1) != Some('.') {
            is_float = true;
            buf.push('.');
            self.bump();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    buf.push(c);
                    self.bump();
                } else if c == '_' {
                    self.bump();
                } else {
                    break;
                }
            }
        }

        if matches!(self.peek(), Some('e' | 'E')) {
            is_float = true;
            buf.push('e');
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                buf.push(self.bump().unwrap());
            }
            let mut exp_digits = 0usize;
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    buf.push(c);
                    exp_digits += 1;
                    self.bump();
                } else if c == '_' {
                    self.bump();
                } else {
                    break;
                }
            }
            if exp_digits == 0 {
                return Err(PyError::syntax(
                    &self.file,
                    start_line,
                    start_col,
                    "invalid float exponent",
                ));
            }
        }

        if is_float {
            let val = buf
                .parse::<f64>()
                .map_err(|_| PyError::syntax(&self.file, start_line, start_col, "invalid float literal"))?;
            Ok(Token {
                kind: TokenKind::Float(val),
                line: start_line,
                col: start_col,
            })
        } else {
            let val = buf
                .parse::<i64>()
                .map_err(|_| PyError::syntax(&self.file, start_line, start_col, "invalid integer literal"))?;
            Ok(Token {
                kind: TokenKind::Int(val),
                line: start_line,
                col: start_col,
            })
        }
    }

    fn lex_operator(&mut self) -> Result<Token, PyError> {
        let line = self.line;
        let col = self.col;
        let ch = self.bump().ok_or_else(|| self.err("unexpected EOF"))?;
        let next = self.peek();
        let next2 = self.peek_at(1);

        let kind = match (ch, next, next2) {
            ('*', Some('*'), Some('=')) => {
                self.bump();
                self.bump();
                TokenKind::DoubleStarEq
            }
            ('/', Some('/'), Some('=')) => {
                self.bump();
                self.bump();
                TokenKind::DoubleSlashEq
            }
            ('<', Some('<'), Some('=')) => {
                self.bump();
                self.bump();
                TokenKind::LShiftEq
            }
            ('>', Some('>'), Some('=')) => {
                self.bump();
                self.bump();
                TokenKind::RShiftEq
            }
            ('.', Some('.'), Some('.')) => {
                self.bump();
                self.bump();
                TokenKind::Ellipsis
            }
            ('*', Some('*'), _) => {
                self.bump();
                TokenKind::DoubleStar
            }
            ('/', Some('/'), _) => {
                self.bump();
                TokenKind::DoubleSlash
            }
            ('<', Some('<'), _) => {
                self.bump();
                TokenKind::LShift
            }
            ('>', Some('>'), _) => {
                self.bump();
                TokenKind::RShift
            }
            ('=', Some('='), _) => {
                self.bump();
                TokenKind::EqEq
            }
            ('!', Some('='), _) => {
                self.bump();
                TokenKind::NotEq
            }
            ('<', Some('='), _) => {
                self.bump();
                TokenKind::LtEq
            }
            ('>', Some('='), _) => {
                self.bump();
                TokenKind::GtEq
            }
            ('+', Some('='), _) => {
                self.bump();
                TokenKind::PlusEq
            }
            ('-', Some('='), _) => {
                self.bump();
                TokenKind::MinusEq
            }
            ('-', Some('>'), _) => {
                self.bump();
                TokenKind::Arrow
            }
            ('*', Some('='), _) => {
                self.bump();
                TokenKind::StarEq
            }
            ('/', Some('='), _) => {
                self.bump();
                TokenKind::SlashEq
            }
            ('%', Some('='), _) => {
                self.bump();
                TokenKind::PercentEq
            }
            ('&', Some('='), _) => {
                self.bump();
                TokenKind::AmpEq
            }
            ('|', Some('='), _) => {
                self.bump();
                TokenKind::PipeEq
            }
            ('^', Some('='), _) => {
                self.bump();
                TokenKind::CaretEq
            }
            (':', Some('='), _) => {
                self.bump();
                TokenKind::ColonEq
            }
            ('+', _, _) => TokenKind::Plus,
            ('-', _, _) => TokenKind::Minus,
            ('*', _, _) => TokenKind::Star,
            ('/', _, _) => TokenKind::Slash,
            ('%', _, _) => TokenKind::Percent,
            ('@', _, _) => TokenKind::At,
            ('=', _, _) => TokenKind::Eq,
            ('<', _, _) => TokenKind::Lt,
            ('>', _, _) => TokenKind::Gt,
            ('&', _, _) => TokenKind::Amp,
            ('|', _, _) => TokenKind::Pipe,
            ('^', _, _) => TokenKind::Caret,
            ('~', _, _) => TokenKind::Tilde,
            ('(', _, _) => {
                self.paren_depth += 1;
                TokenKind::LParen
            }
            (')', _, _) => {
                if self.paren_depth == 0 {
                    return Err(PyError::syntax(&self.file, line, col, "unmatched ')'"));
                }
                self.paren_depth -= 1;
                TokenKind::RParen
            }
            ('[', _, _) => {
                self.paren_depth += 1;
                TokenKind::LBracket
            }
            (']', _, _) => {
                if self.paren_depth == 0 {
                    return Err(PyError::syntax(&self.file, line, col, "unmatched ']'"));
                }
                self.paren_depth -= 1;
                TokenKind::RBracket
            }
            ('{', _, _) => {
                self.paren_depth += 1;
                TokenKind::LBrace
            }
            ('}', _, _) => {
                if self.paren_depth == 0 {
                    return Err(PyError::syntax(&self.file, line, col, "unmatched '}'"));
                }
                self.paren_depth -= 1;
                TokenKind::RBrace
            }
            (',', _, _) => TokenKind::Comma,
            (':', _, _) => TokenKind::Colon,
            ('.', _, _) => TokenKind::Dot,
            (';', _, _) => TokenKind::Semi,
            _ => {
                return Err(PyError::syntax(
                    &self.file,
                    line,
                    col,
                    format!("unexpected character '{ch}'"),
                ));
            }
        };
        Ok(Token { kind, line, col })
    }
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// Decodes Python escape sequences, preserving unknown escapes (e.g. GoldSrc `\y`, `\w`, `\r`?
/// Wait: `\r` is CR in Python! Only unknown escapes like `\y`, `\w`, `\d`, `\R` stay literal!).
pub fn unescape_python_str(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('\'') => out.push('\''),
            Some('"') => out.push('"'),
            Some('0') => out.push('\0'),
            Some('a') => out.push('\x07'),
            Some('b') => out.push('\x08'),
            Some('f') => out.push('\x0C'),
            Some('v') => out.push('\x0B'),
            Some('\n') => {}
            Some('\r') => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            Some('x') => {
                let h1 = chars.next();
                let h2 = chars.next();
                if let (Some(a), Some(b)) = (h1, h2) {
                    let hex = format!("{a}{b}");
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        out.push(byte as char);
                        continue;
                    }
                    out.push_str("\\x");
                    out.push(a);
                    out.push(b);
                } else {
                    out.push_str("\\x");
                    if let Some(a) = h1 {
                        out.push(a);
                    }
                }
            }
            Some('u') => {
                let mut hex = String::with_capacity(4);
                for _ in 0..4 {
                    if let Some(c) = chars.next() {
                        hex.push(c);
                    }
                }
                if hex.len() == 4
                    && let Ok(code) = u32::from_str_radix(&hex, 16)
                    && let Some(u) = char::from_u32(code)
                {
                    out.push(u);
                } else {
                    out.push_str("\\u");
                    out.push_str(&hex);
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}
