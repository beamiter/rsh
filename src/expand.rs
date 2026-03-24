/// Variable, tilde, glob, command substitution, and arithmetic expansion.

use crate::environment::ShellState;
use crate::parser::ast::{Word, WordPart};
use std::process::Command as StdCommand;

/// Expand a Word (Vec<WordPart>) into a list of strings.
/// Word splitting and globbing may produce multiple strings from one Word.
pub fn expand_word(word: &Word, state: &ShellState) -> Vec<String> {
    let expanded = expand_word_to_string(word, state);

    // Glob expansion
    if contains_glob(&expanded) {
        match glob::glob(&expanded) {
            Ok(paths) => {
                let mut results: Vec<String> = paths
                    .filter_map(|p| p.ok())
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                if results.is_empty() {
                    vec![expanded] // No matches: return pattern as-is
                } else {
                    results.sort();
                    results
                }
            }
            Err(_) => vec![expanded],
        }
    } else {
        vec![expanded]
    }
}

/// Expand a Word into a single string (no word splitting/globbing).
pub fn expand_word_to_string(word: &Word, state: &ShellState) -> String {
    let mut result = String::new();
    for part in word {
        result.push_str(&expand_part(part, state));
    }
    result
}

fn expand_part(part: &WordPart, state: &ShellState) -> String {
    match part {
        WordPart::Literal(s) => s.clone(),
        WordPart::SingleQuoted(s) => s.clone(),
        WordPart::DoubleQuoted(parts) => {
            let mut s = String::new();
            for p in parts {
                s.push_str(&expand_part(p, state));
            }
            s
        }
        WordPart::Variable(name) => expand_variable(name, state),
        WordPart::Tilde(user) => expand_tilde(user, state),
        WordPart::Glob(pattern) => pattern.clone(), // returned as-is; expanded at Word level
        WordPart::CommandSub(cmd) => expand_command_sub(cmd),
        WordPart::Arithmetic(expr) => expand_arithmetic(expr, state),
        WordPart::BraceExpansion(_) => String::new(), // TODO
    }
}

fn expand_variable(name: &str, state: &ShellState) -> String {
    match name {
        "?" => state.last_exit_code.to_string(),
        "$" => std::process::id().to_string(),
        "!" => state.last_bg_pid.map_or(String::new(), |p| p.to_string()),
        "#" => String::from("0"), // positional params count - simplified
        "@" | "*" => String::new(), // positional params - simplified
        _ => {
            // Handle ${var:-default}, ${var:=default}, etc.
            if let Some(colon_pos) = name.find(":-") {
                let var_name = &name[..colon_pos];
                let default = &name[colon_pos + 2..];
                match state.get_var(var_name) {
                    Some(v) if !v.is_empty() => v.to_string(),
                    _ => default.to_string(),
                }
            } else if let Some(colon_pos) = name.find(":+") {
                let var_name = &name[..colon_pos];
                let alt = &name[colon_pos + 2..];
                match state.get_var(var_name) {
                    Some(v) if !v.is_empty() => alt.to_string(),
                    _ => String::new(),
                }
            } else {
                state.get_var(name).unwrap_or("").to_string()
            }
        }
    }
}

fn expand_tilde(user: &str, state: &ShellState) -> String {
    if user.is_empty() {
        state.home_dir.to_string_lossy().to_string()
    } else {
        // Try to resolve ~user
        format!("/home/{}", user)
    }
}

fn expand_command_sub(cmd: &str) -> String {
    match StdCommand::new("sh").arg("-c").arg(cmd).output() {
        Ok(output) => {
            let mut s = String::from_utf8_lossy(&output.stdout).to_string();
            // Trim trailing newlines (bash behavior)
            while s.ends_with('\n') || s.ends_with('\r') {
                s.pop();
            }
            s
        }
        Err(_) => String::new(),
    }
}

fn expand_arithmetic(expr: &str, state: &ShellState) -> String {
    // Simple integer arithmetic evaluator
    let expanded = expand_arith_vars(expr, state);
    match eval_arithmetic(&expanded) {
        Ok(n) => n.to_string(),
        Err(_) => String::from("0"),
    }
}

fn expand_arith_vars(expr: &str, state: &ShellState) -> String {
    let mut result = String::new();
    let mut chars = expr.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == '$' {
            chars.next();
            let mut var = String::new();
            while let Some(&c2) = chars.peek() {
                if c2.is_alphanumeric() || c2 == '_' {
                    var.push(c2);
                    chars.next();
                } else {
                    break;
                }
            }
            if !var.is_empty() {
                result.push_str(state.get_var(&var).unwrap_or("0"));
            }
        } else if c.is_alphabetic() || c == '_' {
            // Bare variable name in arithmetic context
            let mut var = String::new();
            while let Some(&c2) = chars.peek() {
                if c2.is_alphanumeric() || c2 == '_' {
                    var.push(c2);
                    chars.next();
                } else {
                    break;
                }
            }
            result.push_str(state.get_var(&var).unwrap_or("0"));
        } else {
            result.push(c);
            chars.next();
        }
    }
    result
}

fn eval_arithmetic(expr: &str) -> Result<i64, String> {
    let tokens = tokenize_arith(expr);
    parse_arith_expr(&tokens, &mut 0)
}

fn tokenize_arith(expr: &str) -> Vec<ArithToken> {
    let mut tokens = Vec::new();
    let mut chars = expr.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => { chars.next(); }
            '0'..='9' => {
                let mut n = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() { n.push(d); chars.next(); } else { break; }
                }
                tokens.push(ArithToken::Num(n.parse().unwrap_or(0)));
            }
            '+' => { chars.next(); tokens.push(ArithToken::Plus); }
            '-' => { chars.next(); tokens.push(ArithToken::Minus); }
            '*' => { chars.next(); tokens.push(ArithToken::Star); }
            '/' => { chars.next(); tokens.push(ArithToken::Slash); }
            '%' => { chars.next(); tokens.push(ArithToken::Percent); }
            '(' => { chars.next(); tokens.push(ArithToken::LParen); }
            ')' => { chars.next(); tokens.push(ArithToken::RParen); }
            '<' => {
                chars.next();
                if chars.peek() == Some(&'=') { chars.next(); tokens.push(ArithToken::Le); }
                else { tokens.push(ArithToken::Lt); }
            }
            '>' => {
                chars.next();
                if chars.peek() == Some(&'=') { chars.next(); tokens.push(ArithToken::Ge); }
                else { tokens.push(ArithToken::Gt); }
            }
            '=' => {
                chars.next();
                if chars.peek() == Some(&'=') { chars.next(); tokens.push(ArithToken::Eq); }
            }
            '!' => {
                chars.next();
                if chars.peek() == Some(&'=') { chars.next(); tokens.push(ArithToken::Ne); }
                else { tokens.push(ArithToken::Not); }
            }
            _ => { chars.next(); }
        }
    }
    tokens
}

#[derive(Debug, Clone)]
enum ArithToken {
    Num(i64), Plus, Minus, Star, Slash, Percent, LParen, RParen,
    Lt, Le, Gt, Ge, Eq, Ne, Not,
}

fn parse_arith_expr(tokens: &[ArithToken], pos: &mut usize) -> Result<i64, String> {
    let mut left = parse_arith_comparison(tokens, pos)?;
    while *pos < tokens.len() {
        match tokens.get(*pos) {
            Some(ArithToken::Plus) => { *pos += 1; left += parse_arith_comparison(tokens, pos)?; }
            Some(ArithToken::Minus) => { *pos += 1; left -= parse_arith_comparison(tokens, pos)?; }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_arith_comparison(tokens: &[ArithToken], pos: &mut usize) -> Result<i64, String> {
    let mut left = parse_arith_term(tokens, pos)?;
    while *pos < tokens.len() {
        match tokens.get(*pos) {
            Some(ArithToken::Lt) => { *pos += 1; let r = parse_arith_term(tokens, pos)?; left = if left < r { 1 } else { 0 }; }
            Some(ArithToken::Le) => { *pos += 1; let r = parse_arith_term(tokens, pos)?; left = if left <= r { 1 } else { 0 }; }
            Some(ArithToken::Gt) => { *pos += 1; let r = parse_arith_term(tokens, pos)?; left = if left > r { 1 } else { 0 }; }
            Some(ArithToken::Ge) => { *pos += 1; let r = parse_arith_term(tokens, pos)?; left = if left >= r { 1 } else { 0 }; }
            Some(ArithToken::Eq) => { *pos += 1; let r = parse_arith_term(tokens, pos)?; left = if left == r { 1 } else { 0 }; }
            Some(ArithToken::Ne) => { *pos += 1; let r = parse_arith_term(tokens, pos)?; left = if left != r { 1 } else { 0 }; }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_arith_term(tokens: &[ArithToken], pos: &mut usize) -> Result<i64, String> {
    let mut left = parse_arith_unary(tokens, pos)?;
    while *pos < tokens.len() {
        match tokens.get(*pos) {
            Some(ArithToken::Star) => { *pos += 1; left *= parse_arith_unary(tokens, pos)?; }
            Some(ArithToken::Slash) => {
                *pos += 1;
                let r = parse_arith_unary(tokens, pos)?;
                if r == 0 { return Err("division by zero".into()); }
                left /= r;
            }
            Some(ArithToken::Percent) => {
                *pos += 1;
                let r = parse_arith_unary(tokens, pos)?;
                if r == 0 { return Err("division by zero".into()); }
                left %= r;
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_arith_unary(tokens: &[ArithToken], pos: &mut usize) -> Result<i64, String> {
    match tokens.get(*pos) {
        Some(ArithToken::Minus) => { *pos += 1; Ok(-parse_arith_primary(tokens, pos)?) }
        Some(ArithToken::Plus) => { *pos += 1; parse_arith_primary(tokens, pos) }
        Some(ArithToken::Not) => { *pos += 1; let v = parse_arith_primary(tokens, pos)?; Ok(if v == 0 { 1 } else { 0 }) }
        _ => parse_arith_primary(tokens, pos),
    }
}

fn parse_arith_primary(tokens: &[ArithToken], pos: &mut usize) -> Result<i64, String> {
    match tokens.get(*pos) {
        Some(ArithToken::Num(n)) => { let n = *n; *pos += 1; Ok(n) }
        Some(ArithToken::LParen) => {
            *pos += 1;
            let v = parse_arith_expr(tokens, pos)?;
            if matches!(tokens.get(*pos), Some(ArithToken::RParen)) {
                *pos += 1;
            }
            Ok(v)
        }
        _ => Ok(0),
    }
}

fn contains_glob(s: &str) -> bool {
    let mut escaped = false;
    let mut in_single = false;
    let mut in_double = false;
    for c in s.chars() {
        if escaped { escaped = false; continue; }
        match c {
            '\\' => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '*' | '?' | '[' if !in_single && !in_double => return true,
            _ => {}
        }
    }
    false
}

/// Expand all words in a command, performing word splitting on the results.
pub fn expand_words(words: &[Word], state: &ShellState) -> Vec<String> {
    let mut result = Vec::new();
    for word in words {
        result.extend(expand_word(word, state));
    }
    result
}
