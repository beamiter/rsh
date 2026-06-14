/// Tokenizer for bash-compatible shell syntax.
/// Supports strict mode (for execution) and lenient mode (for highlighting).

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Word(String),
    Pipe,         // |
    PipeAnd,      // |&
    And,          // &&
    Or,           // ||
    Semi,         // ;
    Amp,          // &
    AmpBang,      // &!  (background + disown)
    RedirectOut,  // >
    RedirectAppend, // >>
    RedirectIn,   // <
    HereDoc,      // <<
    HereDocStrip, // <<-
    HereString,   // <<<
    DupFd,        // >&
    RedirectAllOut, // &> (redirect stdout and stderr)
    RedirectAllAppend, // &>> (append stdout and stderr)
    RedirectFd(i32, RedirectOp),
    LParen,       // (
    RParen,       // )
    LBrace,       // {   (reserved word)
    RBrace,       // }   (reserved word)
    DoubleSemi,   // ;;
    SemiAmp,      // ;&   (case fall-through)
    DoubleSemiAmp,// ;;&  (case continue-match)
    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RedirectOp {
    Output,
    Append,
    Input,
    DupOutput, // N>&M  duplicate output fd
    DupInput,  // N<&M  duplicate input fd
}

#[derive(Debug, Clone)]
pub struct SpannedToken {
    pub token: Token,
    pub span: (usize, usize),
}

pub struct Lexer<'a> {
    input: &'a str,
    pos: usize,
    lenient: bool,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Lexer { input, pos: 0, lenient: false }
    }

    pub fn new_lenient(input: &'a str) -> Self {
        Lexer { input, pos: 0, lenient: true }
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn next_char(&mut self) -> Option<char> {
        let c = self.peek_char()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn peek_char_at(&self, offset: usize) -> Option<char> {
        self.input[self.pos..].chars().nth(offset)
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek_char() {
            if c == ' ' || c == '\t' {
                self.next_char();
            } else if c == '#' {
                // Skip comment to end of line
                while let Some(c) = self.peek_char() {
                    if c == '\n' { break; }
                    self.next_char();
                }
            } else {
                break;
            }
        }
    }

    fn read_single_quoted(&mut self) -> String {
        let mut s = String::new();
        loop {
            match self.next_char() {
                Some('\'') => break,
                Some(c) => s.push(c),
                None => {
                    if !self.lenient {
                        // incomplete
                    }
                    break;
                }
            }
        }
        s
    }

    fn read_double_quoted(&mut self) -> String {
        // Preserve backslash escapes verbatim so the parser's word-part stage can
        // decide how to handle them (a backslash only escapes $ ` " \ newline inside
        // double quotes). Backslash-newline is a line continuation and is removed.
        let mut s = String::new();
        loop {
            match self.next_char() {
                Some('"') => break,
                Some('\\') => {
                    match self.next_char() {
                        Some('\n') => {} // line continuation
                        Some(c) => { s.push('\\'); s.push(c); }
                        None => { s.push('\\'); break; }
                    }
                }
                Some(c) => s.push(c),
                None => break,
            }
        }
        s
    }

    fn read_word(&mut self) -> String {
        let mut word = String::new();
        loop {
            match self.peek_char() {
                None => break,
                Some(c) => match c {
                    ' ' | '\t' | '\n' | '|' | '&' | ';' => break,
                    '(' | ')' => {
                        // Check for array assignment: name=(...)
                        // If word contains '=' and we see '(', continue reading the array
                        if c == '(' && word.contains('=') {
                            // This is array assignment, read until matching )
                            self.next_char();
                            word.push('(');
                            let mut paren_depth = 1;
                            while let Some(c2) = self.peek_char() {
                                if c2 == '(' {
                                    paren_depth += 1;
                                } else if c2 == ')' {
                                    paren_depth -= 1;
                                    if paren_depth == 0 {
                                        self.next_char();
                                        word.push(')');
                                        break;
                                    }
                                }
                                self.next_char();
                                word.push(c2);
                            }
                        } else {
                            break;
                        }
                    }
                    '<' | '>' => {
                        // Check for process substitution <(...) or >(...)
                        if self.peek_char_at(1) == Some('(') {
                            self.next_char(); // consume < or >
                            self.next_char(); // consume (
                            word.push(c);
                            word.push('(');
                            let mut depth = 1;
                            while let Some(c2) = self.next_char() {
                                word.push(c2);
                                if c2 == '(' { depth += 1; }
                                if c2 == ')' {
                                    depth -= 1;
                                    if depth == 0 { break; }
                                }
                            }
                        } else {
                            break; // normal redirect
                        }
                    }
                    '$' if self.peek_char_at(1) == Some('\'') => {
                        // ANSI-C quoting $'...' -- preserve backslash escapes so the
                        // parser can decode them; a backslash-escaped ' does not close.
                        self.next_char(); // $
                        self.next_char(); // '
                        word.push('$');
                        word.push('\'');
                        loop {
                            match self.next_char() {
                                Some('\'') => { word.push('\''); break; }
                                Some('\\') => {
                                    word.push('\\');
                                    if let Some(c2) = self.next_char() { word.push(c2); }
                                }
                                Some(c2) => word.push(c2),
                                None => break,
                            }
                        }
                    }
                    '$' if self.peek_char_at(1) == Some('(') => {
                        // Command substitution $(...) or arithmetic $((...)).
                        // Read the whole parenthesized group (naive paren counting,
                        // matching the process-substitution reader above) so the
                        // word-part parser can decode it later.
                        self.next_char(); // $
                        self.next_char(); // (
                        word.push('$');
                        word.push('(');
                        let mut depth = 1;
                        while let Some(c2) = self.next_char() {
                            word.push(c2);
                            if c2 == '(' { depth += 1; }
                            if c2 == ')' {
                                depth -= 1;
                                if depth == 0 { break; }
                            }
                        }
                    }
                    '\'' => {
                        self.next_char();
                        word.push('\'');
                        let s = self.read_single_quoted();
                        word.push_str(&s);
                        word.push('\'');
                    }
                    '"' => {
                        self.next_char();
                        word.push('"');
                        let s = self.read_double_quoted();
                        word.push_str(&s);
                        word.push('"');
                    }
                    '\\' => {
                        self.next_char();
                        word.push('\\');
                        if let Some(c2) = self.next_char() {
                            word.push(c2);
                        }
                    }
                    '#' if word.is_empty() => break, // comment
                    _ => {
                        self.next_char();
                        word.push(c);
                    }
                }
            }
        }
        word
    }

    pub fn next_token(&mut self) -> SpannedToken {
        self.skip_whitespace();
        let start = self.pos;

        let token = match self.peek_char() {
            None => Token::Eof,
            Some('\n') => {
                self.next_char();
                Token::Newline
            }
            Some('|') => {
                self.next_char();
                match self.peek_char() {
                    Some('|') => { self.next_char(); Token::Or }
                    Some('&') => { self.next_char(); Token::PipeAnd }
                    _ => Token::Pipe,
                }
            }
            Some('&') => {
                self.next_char();
                match self.peek_char() {
                    Some('&') => { self.next_char(); Token::And }
                    Some('!') => { self.next_char(); Token::AmpBang }
                    Some('>') => {
                        self.next_char();
                        match self.peek_char() {
                            Some('>') => { self.next_char(); Token::RedirectAllAppend }
                            _ => Token::RedirectAllOut,
                        }
                    }
                    _ => Token::Amp,
                }
            }
            Some(';') => {
                self.next_char();
                match self.peek_char() {
                    Some(';') => {
                        self.next_char();
                        match self.peek_char() {
                            Some('&') => { self.next_char(); Token::DoubleSemiAmp }
                            _ => Token::DoubleSemi,
                        }
                    }
                    Some('&') => { self.next_char(); Token::SemiAmp }
                    _ => Token::Semi,
                }
            }
            Some('(') => { self.next_char(); Token::LParen }
            Some(')') => { self.next_char(); Token::RParen }
            Some('>') => {
                self.next_char();
                match self.peek_char() {
                    Some('>') => { self.next_char(); Token::RedirectAppend }
                    Some('&') => { self.next_char(); Token::DupFd }
                    Some('(') => {
                        // Process substitution >(cmd) -- back up and read as word
                        self.pos = start;
                        let w = self.read_word();
                        Token::Word(w)
                    }
                    _ => Token::RedirectOut,
                }
            }
            Some('<') => {
                self.next_char();
                match self.peek_char() {
                    Some('<') => {
                        self.next_char();
                        match self.peek_char() {
                            Some('<') => { self.next_char(); Token::HereString }
                            Some('-') => { self.next_char(); Token::HereDocStrip }
                            _ => Token::HereDoc,
                        }
                    }
                    Some('(') => {
                        // Process substitution <(cmd) -- back up and read as word
                        self.pos = start;
                        let w = self.read_word();
                        Token::Word(w)
                    }
                    _ => Token::RedirectIn,
                }
            }
            Some(c) if c.is_ascii_digit() => {
                // Check if it's a redirect like 2> or 2>>
                let saved_pos = self.pos;
                let mut num_str = String::new();
                while let Some(d) = self.peek_char() {
                    if d.is_ascii_digit() {
                        num_str.push(d);
                        self.next_char();
                    } else {
                        break;
                    }
                }
                match self.peek_char() {
                    Some('>') => {
                        let fd: i32 = num_str.parse().unwrap_or(1);
                        self.next_char();
                        match self.peek_char() {
                            Some('>') => { self.next_char(); Token::RedirectFd(fd, RedirectOp::Append) }
                            Some('&') => { self.next_char(); Token::RedirectFd(fd, RedirectOp::DupOutput) }
                            _ => Token::RedirectFd(fd, RedirectOp::Output),
                        }
                    }
                    Some('<') => {
                        let fd: i32 = num_str.parse().unwrap_or(0);
                        self.next_char();
                        match self.peek_char() {
                            Some('&') => { self.next_char(); Token::RedirectFd(fd, RedirectOp::DupInput) }
                            _ => Token::RedirectFd(fd, RedirectOp::Input),
                        }
                    }
                    _ => {
                        // Not a redirect, read as word
                        self.pos = saved_pos;
                        let w = self.read_word();
                        if w == "{" { Token::LBrace }
                        else if w == "}" { Token::RBrace }
                        else { Token::Word(w) }
                    }
                }
            }
            _ => {
                let w = self.read_word();
                if w.is_empty() {
                    // Shouldn't happen but safety
                    self.next_char();
                    Token::Eof
                } else if w == "{" {
                    Token::LBrace
                } else if w == "}" {
                    Token::RBrace
                } else {
                    Token::Word(w)
                }
            }
        };

        SpannedToken { token, span: (start, self.pos) }
    }

    pub fn tokenize_all(&mut self) -> Vec<SpannedToken> {
        let mut tokens = Vec::new();
        loop {
            let t = self.next_token();
            if t.token == Token::Eof {
                tokens.push(t);
                break;
            }
            tokens.push(t);
        }
        tokens
    }

    /// Get the current position in the input stream
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Get the remaining input from current position
    pub fn remaining_input(&self) -> &'a str {
        &self.input[self.pos..]
    }

    /// Set position (used for advancing past here-doc content)
    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }
}

pub fn tokenize(input: &str) -> Vec<SpannedToken> {
    Lexer::new(input).tokenize_all()
}

pub fn tokenize_lenient(input: &str) -> Vec<SpannedToken> {
    Lexer::new_lenient(input).tokenize_all()
}
