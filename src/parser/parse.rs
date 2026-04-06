/// Recursive descent parser for bash-compatible shell grammar.
///
/// Grammar (simplified):
///   program         = complete_command*
///   complete_command = and_or_list ((';' | '&' | '&!') and_or_list)* [';' | '&' | '&!']
///   and_or_list     = pipeline (('&&' | '||') pipeline)*
///   pipeline        = ['!'] command ('|' command)*
///   command         = simple_command | compound_command | function_def
///   simple_command  = (assignment)* word+ (redirect)*

use super::ast::*;
use super::lexer::{Lexer, SpannedToken, Token, RedirectOp};

#[derive(Debug)]
pub enum ParseError {
    Unexpected(String),
    Incomplete,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Unexpected(msg) => write!(f, "syntax error: {}", msg),
            ParseError::Incomplete => write!(f, "incomplete input"),
        }
    }
}

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: SpannedToken,
    peeked: Option<SpannedToken>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        let mut lexer = Lexer::new(input);
        let current = lexer.next_token();
        Parser { lexer, current, peeked: None }
    }

    fn advance(&mut self) {
        if let Some(t) = self.peeked.take() {
            self.current = t;
        } else {
            self.current = self.lexer.next_token();
        }
    }

    fn peek(&mut self) -> &Token {
        if self.peeked.is_none() {
            self.peeked = Some(self.lexer.next_token());
        }
        &self.peeked.as_ref().unwrap().token
    }

    fn expect_word(&mut self) -> Result<String, ParseError> {
        match &self.current.token {
            Token::Word(w) => {
                let w = w.clone();
                self.advance();
                Ok(w)
            }
            Token::Eof => Err(ParseError::Incomplete),
            other => Err(ParseError::Unexpected(format!("expected word, got {:?}", other))),
        }
    }

    fn skip_newlines(&mut self) {
        while self.current.token == Token::Newline {
            self.advance();
        }
    }

    fn is_keyword(&self, kw: &str) -> bool {
        matches!(&self.current.token, Token::Word(w) if w == kw)
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), ParseError> {
        if self.is_keyword(kw) {
            self.advance();
            Ok(())
        } else if self.current.token == Token::Eof {
            Err(ParseError::Incomplete)
        } else {
            Err(ParseError::Unexpected(format!("expected '{}', got {:?}", kw, self.current.token)))
        }
    }

    fn is_redirect(&self) -> bool {
        matches!(self.current.token,
            Token::RedirectOut | Token::RedirectAppend | Token::RedirectIn |
            Token::HereDoc | Token::HereString | Token::DupFd |
            Token::RedirectAllOut | Token::RedirectAllAppend |
            Token::RedirectFd(_, _))
    }

    fn parse_redirect(&mut self) -> Result<Redirect, ParseError> {
        let (fd, kind) = match &self.current.token {
            Token::RedirectOut => (None, RedirectKind::Output),
            Token::RedirectAppend => (None, RedirectKind::Append),
            Token::RedirectIn => (None, RedirectKind::Input),
            Token::HereString => (None, RedirectKind::HereString),
            Token::HereDoc => (None, RedirectKind::HereDoc),
            Token::DupFd => (None, RedirectKind::DupOutput),
            Token::RedirectAllOut => (None, RedirectKind::OutputAll),
            Token::RedirectAllAppend => (None, RedirectKind::AppendAll),
            Token::RedirectFd(n, op) => {
                let fd = Some(*n);
                let kind = match op {
                    RedirectOp::Output => RedirectKind::Output,
                    RedirectOp::Append => RedirectKind::Append,
                    RedirectOp::Input => RedirectKind::Input,
                };
                (fd, kind)
            }
            _ => return Err(ParseError::Unexpected("expected redirect".into())),
        };
        self.advance();
        let target_str = self.expect_word()?;
        let target = parse_word_parts(&target_str);
        Ok(Redirect { fd, kind, target, here_doc: None })
    }

    fn is_command_start(&self) -> bool {
        match &self.current.token {
            Token::Word(w) => !is_list_terminator(w),
            Token::LParen | Token::LBrace => true,
            _ => self.is_redirect(),
        }
    }

    fn parse_simple_command(&mut self) -> Result<Command, ParseError> {
        let mut assignments = Vec::new();
        let mut words: Vec<Word> = Vec::new();
        let mut redirects = Vec::new();

        // Parse leading assignments (VAR=value, arr[idx]=value, arr=(a b c), var+=value)
        loop {
            let is_assign = if let Token::Word(w) = &self.current.token {
                words.is_empty() && is_assignment(w)
            } else {
                false
            };
            if is_assign {
                let w = if let Token::Word(w) = &self.current.token { w.clone() } else { unreachable!() };
                let assign = parse_assignment(&w, self)?;
                assignments.push(assign);
                continue;
            }
            break;
        }

        // Parse words and redirects
        loop {
            if self.is_redirect() {
                redirects.push(self.parse_redirect()?);
            } else if let Token::Word(w) = &self.current.token {
                let w = w.clone();
                words.push(parse_word_parts(&w));
                self.advance();
            } else {
                break;
            }
        }

        if words.is_empty() && assignments.is_empty() && redirects.is_empty() {
            return Err(ParseError::Unexpected(format!(
                "expected command, got {:?}", self.current.token
            )));
        }

        Ok(Command::Simple(SimpleCommand { assignments, words, redirects }))
    }

    fn parse_compound_command(&mut self) -> Result<Command, ParseError> {
        match &self.current.token {
            Token::LParen => self.parse_subshell(),
            Token::LBrace => self.parse_brace_group(),
            Token::Word(w) => {
                match w.as_str() {
                    "if" => self.parse_if(),
                    "for" => self.parse_for(),
                    "while" => self.parse_while(),
                    "until" => self.parse_until(),
                    "case" => self.parse_case(),
                    "select" => self.parse_select(),
                    _ => self.parse_simple_command(),
                }
            }
            _ => self.parse_simple_command(),
        }
    }

    fn parse_subshell(&mut self) -> Result<Command, ParseError> {
        self.advance(); // consume (
        self.skip_newlines();
        let body = self.parse_command_list()?;
        if self.current.token != Token::RParen {
            return Err(if self.current.token == Token::Eof {
                ParseError::Incomplete
            } else {
                ParseError::Unexpected(format!("expected ')', got {:?}", self.current.token))
            });
        }
        self.advance();
        let redirects = self.parse_optional_redirects()?;
        Ok(Command::Compound(CompoundCommand::Subshell { body, redirects }))
    }

    fn parse_brace_group(&mut self) -> Result<Command, ParseError> {
        self.advance(); // consume {
        self.skip_newlines();
        let body = self.parse_command_list()?;
        if self.current.token != Token::RBrace {
            return Err(if self.current.token == Token::Eof {
                ParseError::Incomplete
            } else {
                ParseError::Unexpected(format!("expected '}}', got {:?}", self.current.token))
            });
        }
        self.advance();
        let redirects = self.parse_optional_redirects()?;
        Ok(Command::Compound(CompoundCommand::BraceGroup { body, redirects }))
    }

    fn parse_if(&mut self) -> Result<Command, ParseError> {
        self.advance(); // consume "if"
        self.skip_newlines();
        let mut conditions = Vec::new();
        let condition = self.parse_command_list()?;
        self.expect_keyword("then")?;
        self.skip_newlines();
        let body = self.parse_command_list()?;
        conditions.push((condition, body));

        let mut else_branch = None;
        loop {
            if self.is_keyword("elif") {
                self.advance();
                self.skip_newlines();
                let cond = self.parse_command_list()?;
                self.expect_keyword("then")?;
                self.skip_newlines();
                let body = self.parse_command_list()?;
                conditions.push((cond, body));
            } else if self.is_keyword("else") {
                self.advance();
                self.skip_newlines();
                else_branch = Some(self.parse_command_list()?);
                break;
            } else {
                break;
            }
        }
        self.expect_keyword("fi")?;
        let redirects = self.parse_optional_redirects()?;
        Ok(Command::Compound(CompoundCommand::If { conditions, else_branch, redirects }))
    }

    fn parse_for(&mut self) -> Result<Command, ParseError> {
        self.advance(); // consume "for"
        let var = self.expect_word()?;
        self.skip_newlines();

        let words = if self.is_keyword("in") {
            self.advance();
            let mut ws = Vec::new();
            while let Token::Word(w) = &self.current.token {
                let w = w.clone();
                ws.push(parse_word_parts(&w));
                self.advance();
            }
            // consume separator
            if self.current.token == Token::Semi || self.current.token == Token::Newline {
                self.advance();
            }
            Some(ws)
        } else {
            if self.current.token == Token::Semi || self.current.token == Token::Newline {
                self.advance();
            }
            None
        };

        self.skip_newlines();
        self.expect_keyword("do")?;
        self.skip_newlines();
        let body = self.parse_command_list()?;
        self.expect_keyword("done")?;
        let redirects = self.parse_optional_redirects()?;
        Ok(Command::Compound(CompoundCommand::For { var, words, body, redirects }))
    }

    fn parse_while(&mut self) -> Result<Command, ParseError> {
        self.advance(); // consume "while"
        self.skip_newlines();
        let condition = self.parse_command_list()?;
        self.expect_keyword("do")?;
        self.skip_newlines();
        let body = self.parse_command_list()?;
        self.expect_keyword("done")?;
        let redirects = self.parse_optional_redirects()?;
        Ok(Command::Compound(CompoundCommand::While { condition, body, redirects }))
    }

    fn parse_until(&mut self) -> Result<Command, ParseError> {
        self.advance(); // consume "until"
        self.skip_newlines();
        let condition = self.parse_command_list()?;
        self.expect_keyword("do")?;
        self.skip_newlines();
        let body = self.parse_command_list()?;
        self.expect_keyword("done")?;
        let redirects = self.parse_optional_redirects()?;
        Ok(Command::Compound(CompoundCommand::Until { condition, body, redirects }))
    }

    fn parse_case(&mut self) -> Result<Command, ParseError> {
        self.advance(); // consume "case"
        let word_str = self.expect_word()?;
        let word = parse_word_parts(&word_str);
        self.skip_newlines();
        self.expect_keyword("in")?;
        self.skip_newlines();

        let mut arms = Vec::new();
        while !self.is_keyword("esac") && self.current.token != Token::Eof {
            // optional (
            if self.current.token == Token::LParen {
                self.advance();
            }
            let mut patterns = Vec::new();
            let p = self.expect_word()?;
            patterns.push(parse_word_parts(&p));
            while let Token::Pipe = &self.current.token {
                self.advance();
                let p = self.expect_word()?;
                patterns.push(parse_word_parts(&p));
            }
            if self.current.token != Token::RParen {
                return Err(ParseError::Unexpected(format!("expected ')' in case, got {:?}", self.current.token)));
            }
            self.advance();
            self.skip_newlines();
            let body = self.parse_command_list()?;
            if self.current.token == Token::DoubleSemi {
                self.advance();
                self.skip_newlines();
            }
            arms.push(CaseArm { patterns, body });
        }
        self.expect_keyword("esac")?;
        let redirects = self.parse_optional_redirects()?;
        Ok(Command::Compound(CompoundCommand::Case { word, arms, redirects }))
    }

    fn parse_select(&mut self) -> Result<Command, ParseError> {
        self.advance(); // consume "select"
        let var = self.expect_word()?;
        self.skip_newlines();

        let words = if self.is_keyword("in") {
            self.advance();
            let mut ws = Vec::new();
            while let Token::Word(w) = &self.current.token {
                let w = w.clone();
                ws.push(parse_word_parts(&w));
                self.advance();
            }
            // consume separator
            if self.current.token == Token::Semi || self.current.token == Token::Newline {
                self.advance();
            }
            Some(ws)
        } else {
            if self.current.token == Token::Semi || self.current.token == Token::Newline {
                self.advance();
            }
            None
        };

        self.skip_newlines();
        self.expect_keyword("do")?;
        self.skip_newlines();
        let body = self.parse_command_list()?;
        self.expect_keyword("done")?;
        let redirects = self.parse_optional_redirects()?;
        Ok(Command::Compound(CompoundCommand::Select { var, words, body, redirects }))
    }

    fn parse_optional_redirects(&mut self) -> Result<Vec<Redirect>, ParseError> {
        let mut redirects = Vec::new();
        while self.is_redirect() {
            redirects.push(self.parse_redirect()?);
        }
        Ok(redirects)
    }

    fn parse_command(&mut self) -> Result<Command, ParseError> {
        // Check for function definition: name() { ... }
        if let Token::Word(name) = &self.current.token {
            let name = name.clone();
            if !is_reserved_word(&name) {
                if let Token::LParen = self.peek() {
                    // Could be function def
                    let saved = self.current.clone();
                    self.advance(); // consume name
                    if self.current.token == Token::LParen {
                        self.advance(); // consume (
                        if self.current.token == Token::RParen {
                            self.advance(); // consume )
                            self.skip_newlines();
                            let body = self.parse_compound_command()?;
                            if let Command::Compound(c) = body {
                                return Ok(Command::FunctionDef { name, body: Box::new(c) });
                            }
                        }
                    }
                    // Restore - not a function def
                    self.current = saved;
                }
            }
        }

        // Check for compound commands
        match &self.current.token {
            Token::LParen | Token::LBrace => self.parse_compound_command(),
            Token::Word(w) if is_compound_keyword(w) => self.parse_compound_command(),
            Token::Word(w) if w == "function" => {
                self.advance(); // consume "function"
                let name = self.expect_word()?;
                // optional ()
                if self.current.token == Token::LParen {
                    self.advance();
                    if self.current.token == Token::RParen {
                        self.advance();
                    }
                }
                self.skip_newlines();
                let body = self.parse_compound_command()?;
                if let Command::Compound(c) = body {
                    Ok(Command::FunctionDef { name, body: Box::new(c) })
                } else {
                    Err(ParseError::Unexpected("expected compound command after function name".into()))
                }
            }
            _ => self.parse_simple_command(),
        }
    }

    fn parse_pipeline(&mut self) -> Result<Pipeline, ParseError> {
        let negated = if self.is_keyword("!") {
            self.advance();
            true
        } else {
            false
        };

        let mut commands = vec![self.parse_command()?];

        while self.current.token == Token::Pipe || self.current.token == Token::PipeAnd {
            self.advance();
            self.skip_newlines();
            commands.push(self.parse_command()?);
        }

        Ok(Pipeline { negated, commands })
    }

    fn parse_and_or(&mut self) -> Result<AndOrList, ParseError> {
        let first = self.parse_pipeline()?;
        let mut rest = Vec::new();

        loop {
            let conn = match &self.current.token {
                Token::And => Connector::And,
                Token::Or => Connector::Or,
                _ => break,
            };
            self.advance();
            self.skip_newlines();
            rest.push((conn, self.parse_pipeline()?));
        }

        Ok(AndOrList { first, rest })
    }

    fn parse_complete_command(&mut self) -> Result<CompleteCommand, ParseError> {
        let list = self.parse_and_or()?;
        let (background, disown) = match &self.current.token {
            Token::Amp => { self.advance(); (true, false) }
            Token::AmpBang => { self.advance(); (true, true) }
            _ => (false, false),
        };
        Ok(CompleteCommand { list, background, disown })
    }

    fn parse_command_list(&mut self) -> Result<Vec<CompleteCommand>, ParseError> {
        let mut commands = Vec::new();
        self.skip_newlines();

        while self.is_command_start() {
            commands.push(self.parse_complete_command()?);
            // consume separators
            while self.current.token == Token::Semi || self.current.token == Token::Newline {
                self.advance();
            }
        }

        Ok(commands)
    }

    pub fn parse_program(&mut self) -> Result<Vec<CompleteCommand>, ParseError> {
        let cmds = self.parse_command_list()?;
        if self.current.token != Token::Eof {
            return Err(ParseError::Unexpected(format!(
                "unexpected token {:?}", self.current.token
            )));
        }
        Ok(cmds)
    }
}

/// Check if input is syntactically incomplete (for multiline editing).
pub fn is_incomplete(input: &str) -> bool {
    // Check for unclosed quotes
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for c in input.chars() {
        if escaped { escaped = false; continue; }
        match c {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            _ => {}
        }
    }
    if in_single || in_double { return true; }

    // Check for trailing pipe, &&, ||, or backslash
    let trimmed = input.trim_end();
    if trimmed.ends_with('|') || trimmed.ends_with("&&") || trimmed.ends_with("||") || trimmed.ends_with('\\') {
        return true;
    }

    // Try parsing - if Incomplete, return true
    let mut parser = Parser::new(input);
    matches!(parser.parse_program(), Err(ParseError::Incomplete))
}

pub fn parse(input: &str) -> Result<Vec<CompleteCommand>, ParseError> {
    Parser::new(input).parse_program()
}

// --- Helper functions ---

fn is_reserved_word(w: &str) -> bool {
    matches!(w, "if" | "then" | "else" | "elif" | "fi" | "for" | "in" | "do" | "done" |
             "while" | "until" | "case" | "esac" | "function" | "!" | "{" | "}")
}

fn is_compound_keyword(w: &str) -> bool {
    matches!(w, "if" | "for" | "while" | "until" | "case" | "select")
}

/// Keywords that terminate a command list (not valid as the start of a new command).
fn is_list_terminator(w: &str) -> bool {
    matches!(w, "then" | "else" | "elif" | "fi" | "do" | "done" | "esac" | "}" | ")")
}

fn is_assignment(w: &str) -> bool {
    // Support: VAR=value, VAR+=value, arr[idx]=value, arr=(...)
    let w_bytes = w.as_bytes();

    // Find the = sign (or += pattern)
    let mut i = 0;
    while i < w_bytes.len() {
        let c = w_bytes[i] as char;
        if c == '=' {
            // Everything before = must be a valid name (possibly with [idx])
            let before = &w[..i];
            return is_valid_assign_lhs(before);
        }
        if c == '+' && i + 1 < w_bytes.len() && w_bytes[i + 1] == b'=' {
            let before = &w[..i];
            return is_valid_assign_lhs(before);
        }
        if c == '[' {
            // skip to ] for array index
            while i < w_bytes.len() && w_bytes[i] != b']' {
                i += 1;
            }
        }
        i += 1;
    }
    false
}

fn is_valid_assign_lhs(s: &str) -> bool {
    if s.is_empty() { return false; }
    // Could be "name" or "name[idx]"
    let name = if let Some(bracket) = s.find('[') {
        if !s.ends_with(']') { return false; }
        &s[..bracket]
    } else {
        s
    };
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit())
}

fn parse_assignment(w: &str, parser: &mut Parser) -> Result<Assignment, ParseError> {
    // Detect += vs =
    let (before_eq, value_str, append) = if let Some(pos) = w.find("+=") {
        (&w[..pos], &w[pos + 2..], true)
    } else {
        let eq_pos = w.find('=').unwrap();
        (&w[..eq_pos], &w[eq_pos + 1..], false)
    };

    // Extract name and optional index from lhs
    let (name, index) = if let Some(bracket) = before_eq.find('[') {
        let idx = &before_eq[bracket + 1..before_eq.len() - 1]; // strip [ ]
        (&before_eq[..bracket], Some(idx.to_string()))
    } else {
        (before_eq, None)
    };

    // Check for array literal: name=(a b c)
    if value_str == "(" || (value_str.starts_with('(') && !value_str.ends_with(')')) {
        // Collect words until )
        let mut array_words = Vec::new();
        // If value_str is just "(", we need to read from parser
        let inner = if value_str == "(" {
            String::new()
        } else {
            value_str[1..].to_string()
        };

        // Parse any words already in the token
        if !inner.is_empty() {
            for part in inner.split_whitespace() {
                array_words.push(parse_word_parts(part));
            }
        }

        parser.advance(); // consume the assignment token

        // Read more words until )
        loop {
            match &parser.current.token {
                Token::RParen => {
                    parser.advance();
                    break;
                }
                Token::Word(w) => {
                    let w = w.clone();
                    // Check if word ends with )
                    if w.ends_with(')') {
                        let inner = &w[..w.len() - 1];
                        if !inner.is_empty() {
                            array_words.push(parse_word_parts(inner));
                        }
                        parser.advance();
                        break;
                    }
                    array_words.push(parse_word_parts(&w));
                    parser.advance();
                }
                Token::Eof => return Err(ParseError::Incomplete),
                _ => {
                    parser.advance();
                    break;
                }
            }
        }

        return Ok(Assignment {
            name: name.to_string(),
            value: vec![WordPart::Literal(String::new())],
            index: None,
            append,
            array_value: Some(array_words),
        });
    }

    // Check for complete array literal: name=(a b c) all in one token
    if value_str.starts_with('(') && value_str.ends_with(')') {
        let inner = &value_str[1..value_str.len() - 1];
        let array_words: Vec<Word> = inner.split_whitespace()
            .map(|s| parse_word_parts(s))
            .collect();
        parser.advance();
        return Ok(Assignment {
            name: name.to_string(),
            value: vec![WordPart::Literal(String::new())],
            index: None,
            append,
            array_value: Some(array_words),
        });
    }

    parser.advance();
    Ok(Assignment {
        name: name.to_string(),
        value: parse_word_parts(value_str),
        index,
        append,
        array_value: None,
    })
}

/// Parse a raw word string into WordPart components.
pub fn parse_word_parts(raw: &str) -> Word {
    let mut parts = Vec::new();
    let mut chars = raw.chars().peekable();
    let mut literal = String::new();

    while let Some(&c) = chars.peek() {
        match c {
            '\'' => {
                if !literal.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                }
                chars.next();
                let mut s = String::new();
                while let Some(&c2) = chars.peek() {
                    if c2 == '\'' { chars.next(); break; }
                    s.push(c2);
                    chars.next();
                }
                parts.push(WordPart::SingleQuoted(s));
            }
            '"' => {
                if !literal.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                }
                chars.next();
                let mut inner = String::new();
                while let Some(&c2) = chars.peek() {
                    if c2 == '"' { chars.next(); break; }
                    if c2 == '\\' {
                        chars.next();
                        if let Some(&c3) = chars.peek() {
                            inner.push(c3);
                            chars.next();
                        }
                        continue;
                    }
                    inner.push(c2);
                    chars.next();
                }
                // Parse inner for variables
                parts.push(WordPart::DoubleQuoted(parse_word_parts_inner(&inner)));
            }
            '$' => {
                if !literal.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                }
                chars.next();
                match chars.peek() {
                    Some(&'(') => {
                        chars.next();
                        if chars.peek() == Some(&'(') {
                            // Arithmetic $((...))
                            chars.next();
                            let mut expr = String::new();
                            let mut depth = 1;
                            while let Some(&c2) = chars.peek() {
                                if c2 == ')' {
                                    chars.next();
                                    if chars.peek() == Some(&')') {
                                        chars.next();
                                        depth -= 1;
                                        if depth == 0 { break; }
                                    }
                                    expr.push(')');
                                } else if c2 == '(' {
                                    chars.next();
                                    if chars.peek() == Some(&'(') {
                                        chars.next();
                                        depth += 1;
                                        expr.push_str("((");
                                    } else {
                                        expr.push('(');
                                    }
                                } else {
                                    expr.push(c2);
                                    chars.next();
                                }
                            }
                            parts.push(WordPart::Arithmetic(expr));
                        } else {
                            // Command substitution $(...)
                            let mut cmd = String::new();
                            let mut depth = 1;
                            while let Some(&c2) = chars.peek() {
                                if c2 == '(' { depth += 1; }
                                if c2 == ')' {
                                    depth -= 1;
                                    if depth == 0 { chars.next(); break; }
                                }
                                cmd.push(c2);
                                chars.next();
                            }
                            parts.push(WordPart::CommandSub(cmd));
                        }
                    }
                    Some(&'{') => {
                        chars.next();
                        let mut var = String::new();
                        while let Some(&c2) = chars.peek() {
                            if c2 == '}' { chars.next(); break; }
                            var.push(c2);
                            chars.next();
                        }
                        parts.push(WordPart::Variable(var));
                    }
                    Some(&c2) if c2.is_alphanumeric() || c2 == '_' || c2 == '?' || c2 == '$' || c2 == '!' || c2 == '#' || c2 == '@' || c2 == '*' => {
                        let mut var = String::new();
                        if "?$!#@*".contains(c2) {
                            var.push(c2);
                            chars.next();
                        } else {
                            while let Some(&c3) = chars.peek() {
                                if c3.is_alphanumeric() || c3 == '_' {
                                    var.push(c3);
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                        }
                        parts.push(WordPart::Variable(var));
                    }
                    _ => {
                        literal.push('$');
                    }
                }
            }
            '`' => {
                if !literal.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                }
                chars.next();
                let mut cmd = String::new();
                while let Some(&c2) = chars.peek() {
                    if c2 == '`' { chars.next(); break; }
                    if c2 == '\\' {
                        chars.next();
                        if let Some(&c3) = chars.peek() {
                            cmd.push(c3);
                            chars.next();
                        }
                        continue;
                    }
                    cmd.push(c2);
                    chars.next();
                }
                parts.push(WordPart::CommandSub(cmd));
            }
            '~' if literal.is_empty() && parts.is_empty() => {
                chars.next();
                let mut user = String::new();
                while let Some(&c2) = chars.peek() {
                    if c2 == '/' || c2 == ':' { break; }
                    user.push(c2);
                    chars.next();
                }
                parts.push(WordPart::Tilde(user));
            }
            '<' | '>' if chars.clone().nth(1) == Some('(') => {
                // Process substitution: <(cmd) or >(cmd)
                if !literal.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                }
                let kind = if c == '<' { ProcessSubKind::Input } else { ProcessSubKind::Output };
                chars.next(); // consume < or >
                chars.next(); // consume (
                let mut cmd = String::new();
                let mut depth = 1;
                while let Some(&c2) = chars.peek() {
                    if c2 == '(' { depth += 1; }
                    if c2 == ')' {
                        depth -= 1;
                        if depth == 0 { chars.next(); break; }
                    }
                    cmd.push(c2);
                    chars.next();
                }
                parts.push(WordPart::ProcessSub(cmd, kind));
            }
            '*' | '?' | '[' => {
                if !literal.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                }
                let mut glob = String::new();
                glob.push(c);
                chars.next();
                if c == '[' {
                    while let Some(&c2) = chars.peek() {
                        glob.push(c2);
                        chars.next();
                        if c2 == ']' { break; }
                    }
                }
                parts.push(WordPart::Glob(glob));
            }
            '\\' => {
                chars.next();
                if let Some(&c2) = chars.peek() {
                    literal.push(c2);
                    chars.next();
                }
            }
            '{' => {
                // Try brace expansion: {a,b,c} or {1..10}
                if !literal.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                }
                chars.next(); // consume '{'
                let mut content = String::new();
                let mut depth = 1;
                let mut found_close = false;
                let save = chars.clone();
                while let Some(&c2) = chars.peek() {
                    if c2 == '{' { depth += 1; }
                    if c2 == '}' {
                        depth -= 1;
                        if depth == 0 { chars.next(); found_close = true; break; }
                    }
                    content.push(c2);
                    chars.next();
                }
                if !found_close {
                    // No closing brace - treat as literal
                    literal.push('{');
                    literal.push_str(&content);
                    chars = save;
                    // Consume all characters we already consumed from save
                    for _ in 0..content.len() { chars.next(); }
                    continue;
                }
                // Check if it's a range: start..end[..step]
                if let Some(range) = parse_brace_range(&content) {
                    parts.push(range);
                } else if content.contains(',') {
                    // Comma-separated brace expansion: {a,b,c}
                    let items: Vec<Vec<WordPart>> = content.split(',')
                        .map(|s| parse_word_parts(s))
                        .collect();
                    parts.push(WordPart::BraceExpansion(items));
                } else {
                    // Not a valid brace expansion - treat as literal
                    literal.push('{');
                    literal.push_str(&content);
                    literal.push('}');
                }
            }
            _ => {
                literal.push(c);
                chars.next();
            }
        }
    }

    if !literal.is_empty() {
        parts.push(WordPart::Literal(literal));
    }

    if parts.is_empty() {
        parts.push(WordPart::Literal(String::new()));
    }

    parts
}

fn parse_word_parts_inner(input: &str) -> Vec<WordPart> {
    let mut parts = Vec::new();
    let mut chars = input.chars().peekable();
    let mut literal = String::new();

    while let Some(&c) = chars.peek() {
        if c == '$' {
            if !literal.is_empty() {
                parts.push(WordPart::Literal(std::mem::take(&mut literal)));
            }
            chars.next();
            match chars.peek() {
                Some(&c2) if c2.is_alphanumeric() || c2 == '_' || c2 == '?' || c2 == '$' || c2 == '!' || c2 == '#' || c2 == '@' || c2 == '*' => {
                    let mut var = String::new();
                    if "?$!#@*".contains(c2) {
                        var.push(c2);
                        chars.next();
                    } else {
                        while let Some(&c3) = chars.peek() {
                            if c3.is_alphanumeric() || c3 == '_' {
                                var.push(c3);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                    }
                    parts.push(WordPart::Variable(var));
                }
                Some(&'{') => {
                    chars.next();
                    let mut var = String::new();
                    while let Some(&c2) = chars.peek() {
                        if c2 == '}' { chars.next(); break; }
                        var.push(c2);
                        chars.next();
                    }
                    parts.push(WordPart::Variable(var));
                }
                Some(&'(') => {
                    chars.next();
                    let mut cmd = String::new();
                    let mut depth = 1;
                    while let Some(&c2) = chars.peek() {
                        if c2 == '(' { depth += 1; }
                        if c2 == ')' { depth -= 1; if depth == 0 { chars.next(); break; } }
                        cmd.push(c2);
                        chars.next();
                    }
                    parts.push(WordPart::CommandSub(cmd));
                }
                _ => literal.push('$'),
            }
        } else {
            literal.push(c);
            chars.next();
        }
    }

    if !literal.is_empty() {
        parts.push(WordPart::Literal(literal));
    }
    parts
}

fn parse_brace_range(content: &str) -> Option<WordPart> {
    let parts: Vec<&str> = content.split("..").collect();
    if parts.len() == 2 {
        Some(WordPart::BraceRange {
            start: parts[0].to_string(),
            end: parts[1].to_string(),
            step: None,
        })
    } else if parts.len() == 3 {
        Some(WordPart::BraceRange {
            start: parts[0].to_string(),
            end: parts[1].to_string(),
            step: Some(parts[2].to_string()),
        })
    } else {
        None
    }
}
