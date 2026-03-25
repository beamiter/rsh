/// Variable, tilde, glob, command substitution, and arithmetic expansion.

use crate::environment::ShellState;
use crate::parser::ast::{Word, WordPart};

/// Expand a Word (Vec<WordPart>) into a list of strings.
/// Word splitting and globbing may produce multiple strings from one Word.
pub fn expand_word(word: &Word, state: &mut ShellState) -> Vec<String> {
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
pub fn expand_word_to_string(word: &Word, state: &mut ShellState) -> String {
    let mut result = String::new();
    for part in word {
        result.push_str(&expand_part(part, state));
    }
    result
}

fn expand_part(part: &WordPart, state: &mut ShellState) -> String {
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
        WordPart::CommandSub(cmd) => expand_command_sub(cmd, state),
        WordPart::Arithmetic(expr) => expand_arithmetic(expr, state),
        WordPart::BraceExpansion(items) => {
            // Single-part fallback: expand and join (full multi-word handled in expand_word)
            expand_brace_items(items, state).join(" ")
        }
    }
}

fn expand_variable(name: &str, state: &mut ShellState) -> String {
    match name {
        "?" => state.last_exit_code.to_string(),
        "$" => std::process::id().to_string(),
        "!" => state.last_bg_pid.map_or(String::new(), |p| p.to_string()),
        "#" => state.positional_params.len().to_string(),
        "@" | "*" => state.positional_params.join(" "),
        "0" => std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "rsh".into()),
        _ if name.len() <= 3 && name.chars().all(|c| c.is_ascii_digit()) => {
            let idx: usize = name.parse().unwrap_or(0);
            if idx > 0 && idx <= state.positional_params.len() {
                state.positional_params[idx - 1].clone()
            } else {
                String::new()
            }
        }
        _ => {
            expand_parameter(name, state)
        }
    }
}

fn expand_parameter(name: &str, state: &mut ShellState) -> String {
    // ${var:-default}
    if let Some(pos) = name.find(":-") {
        let var = &name[..pos];
        let default = &name[pos + 2..];
        return match state.get_var(var) {
            Some(v) if !v.is_empty() => v.to_string(),
            _ => default.to_string(),
        };
    }
    // ${var:=default} (assign default)
    if let Some(pos) = name.find(":=") {
        let var = &name[..pos];
        let default = &name[pos + 2..];
        return match state.get_var(var) {
            Some(v) if !v.is_empty() => v.to_string(),
            _ => {
                let val = default.to_string();
                state.set_var(var, &val);
                val
            }
        };
    }
    // ${var:+alternate}
    if let Some(pos) = name.find(":+") {
        let var = &name[..pos];
        let alt = &name[pos + 2..];
        return match state.get_var(var) {
            Some(v) if !v.is_empty() => alt.to_string(),
            _ => String::new(),
        };
    }
    // ${var:offset:length} and ${var:offset}
    if let Some(pos) = name.find(':') {
        let var = &name[..pos];
        let rest = &name[pos + 1..];
        // Check it's numeric (substring operation)
        if rest.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
            let val = state.get_var(var).unwrap_or("");
            if let Some(colon2) = rest.find(':') {
                let offset: i64 = rest[..colon2].parse().unwrap_or(0);
                let length: usize = rest[colon2 + 1..].parse().unwrap_or(val.len());
                let start = if offset < 0 { (val.len() as i64 + offset).max(0) as usize } else { offset as usize };
                let end = (start + length).min(val.len());
                return val.get(start..end).unwrap_or("").to_string();
            } else {
                let offset: i64 = rest.parse().unwrap_or(0);
                let start = if offset < 0 { (val.len() as i64 + offset).max(0) as usize } else { offset as usize };
                return val.get(start..).unwrap_or("").to_string();
            }
        }
    }
    // ${var##pattern} (greedy prefix strip)
    if let Some(pos) = name.find("##") {
        let var = &name[..pos];
        let pat = &name[pos + 2..];
        let val = state.get_var(var).unwrap_or("");
        for i in (0..=val.len()).rev() {
            if match_glob(pat, &val[..i]) { return val[i..].to_string(); }
        }
        return val.to_string();
    }
    // ${var#pattern} (shortest prefix strip)
    if let Some(pos) = name.find('#') {
        let var = &name[..pos];
        let pat = &name[pos + 1..];
        let val = state.get_var(var).unwrap_or("");
        for i in 0..=val.len() {
            if match_glob(pat, &val[..i]) { return val[i..].to_string(); }
        }
        return val.to_string();
    }
    // ${var%%pattern} (greedy suffix strip)
    if let Some(pos) = name.find("%%") {
        let var = &name[..pos];
        let pat = &name[pos + 2..];
        let val = state.get_var(var).unwrap_or("");
        for i in 0..=val.len() {
            if match_glob(pat, &val[i..]) { return val[..i].to_string(); }
        }
        return val.to_string();
    }
    // ${var%pattern} (shortest suffix strip)
    if let Some(pos) = name.find('%') {
        let var = &name[..pos];
        let pat = &name[pos + 1..];
        let val = state.get_var(var).unwrap_or("");
        for i in (0..=val.len()).rev() {
            if match_glob(pat, &val[i..]) { return val[..i].to_string(); }
        }
        return val.to_string();
    }
    // ${var//pattern/replacement} (global replace)
    if let Some(pos) = name.find("//") {
        let var = &name[..pos];
        let rest = &name[pos + 2..];
        let (pat, rep) = rest.split_once('/').unwrap_or((rest, ""));
        let val = state.get_var(var).unwrap_or("");
        return val.replace(pat, rep);
    }
    // ${var/pattern/replacement} (first replace)
    if let Some(pos) = name.find('/') {
        let var = &name[..pos];
        let rest = &name[pos + 1..];
        let (pat, rep) = rest.split_once('/').unwrap_or((rest, ""));
        let val = state.get_var(var).unwrap_or("");
        return val.replacen(pat, rep, 1);
    }
    // ${#var} (string length)
    if let Some(var) = name.strip_prefix('#') {
        let val = state.get_var(var).unwrap_or("");
        return val.len().to_string();
    }
    state.get_var(name).unwrap_or("").to_string()
}

fn expand_brace_items(items: &[Vec<WordPart>], state: &mut ShellState) -> Vec<String> {
    items.iter().map(|parts| {
        let mut s = String::new();
        for p in parts { s.push_str(&expand_part(p, state)); }
        s
    }).collect()
}

fn match_glob(pattern: &str, text: &str) -> bool {
    crate::glob_match::glob_match(pattern, text)
}

fn expand_tilde(user: &str, state: &mut ShellState) -> String {
    if user.is_empty() {
        state.home_dir.to_string_lossy().to_string()
    } else {
        // Resolve ~user via passwd lookup
        let c_user = std::ffi::CString::new(user).unwrap_or_default();
        let pw = unsafe { nix::libc::getpwnam(c_user.as_ptr()) };
        if pw.is_null() {
            format!("~{}", user)
        } else {
            let dir = unsafe { std::ffi::CStr::from_ptr((*pw).pw_dir) };
            dir.to_string_lossy().to_string()
        }
    }
}

fn expand_command_sub(cmd: &str, state: &mut crate::environment::ShellState) -> String {
    // Fork and capture stdout in-process, avoiding re-exec of rsh binary.
    use nix::unistd::{close, fork, pipe, read, ForkResult};
    use std::os::unix::io::IntoRawFd;

    let (r, w) = match pipe() {
        Ok(fds) => (fds.0.into_raw_fd(), fds.1.into_raw_fd()),
        Err(_) => return String::new(),
    };

    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            close(r).ok();
            // Redirect stdout to write end of pipe
            nix::unistd::dup2(w, 1).ok();
            close(w).ok();

            // Parse and execute inside the child (inherits parent state via fork COW)
            state.interactive = false;
            match crate::parser::parse(cmd) {
                Ok(cmds) => {
                    let mut code = 0;
                    for c in &cmds {
                        code = crate::executor::execute_complete_command(c, state);
                    }
                    std::process::exit(code);
                }
                Err(_) => std::process::exit(2),
            }
        }
        Ok(ForkResult::Parent { child }) => {
            close(w).ok();
            // Read all output from child
            let mut output = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match read(r, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => output.extend_from_slice(&buf[..n]),
                }
            }
            close(r).ok();
            nix::sys::wait::waitpid(child, None).ok();
            let mut s = String::from_utf8_lossy(&output).to_string();
            // Trim trailing newlines (bash behavior)
            while s.ends_with('\n') || s.ends_with('\r') {
                s.pop();
            }
            s
        }
        Err(_) => {
            close(r).ok();
            close(w).ok();
            String::new()
        }
    }
}

fn expand_arithmetic(expr: &str, state: &mut ShellState) -> String {
    // Simple integer arithmetic evaluator
    let expanded = expand_arith_vars(expr, state);
    match eval_arithmetic(&expanded) {
        Ok(n) => n.to_string(),
        Err(_) => String::from("0"),
    }
}

fn expand_arith_vars(expr: &str, state: &mut ShellState) -> String {
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
pub fn expand_words(words: &[Word], state: &mut ShellState) -> Vec<String> {
    let mut result = Vec::new();
    for word in words {
        result.extend(expand_word(word, state));
    }
    result
}
