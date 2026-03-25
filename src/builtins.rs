/// Built-in shell commands.

use crate::environment::ShellState;
use crate::parser;
use std::env;
use std::path::Path;

pub fn is_builtin(name: &str) -> bool {
    matches!(name,
        "cd" | "exit" | "export" | "unset" | "echo" | "printf" | "pwd" |
        "alias" | "unalias" | "type" | "source" | "." | "eval" | "read" |
        "true" | "false" | "test" | "[" | "return" | "break" | "continue" |
        "shift" | "set" | "local" | "jobs" | "fg" | "bg" | "history" | "help" |
        "pushd" | "popd" | "dirs" | "trap" | "command" | "builtin" | "[["
    )
}

pub fn run_builtin(name: &str, args: &[String], state: &mut ShellState) -> i32 {
    match name {
        "cd" => builtin_cd(args, state),
        "exit" => builtin_exit(args),
        "export" => builtin_export(args, state),
        "unset" => builtin_unset(args, state),
        "echo" => builtin_echo(args),
        "printf" => builtin_printf(args),
        "pwd" => builtin_pwd(),
        "alias" => builtin_alias(args, state),
        "unalias" => builtin_unalias(args, state),
        "type" => builtin_type(args, state),
        "source" | "." => builtin_source(args, state),
        "eval" => builtin_eval(args, state),
        "read" => builtin_read(args, state),
        "true" => 0,
        "false" => 1,
        "test" | "[" => builtin_test(args),
        "set" => builtin_set(args, state),
        "local" => builtin_local(args, state),
        "return" | "break" | "continue" => 0, // simplified
        "shift" => builtin_shift(state),
        "help" => { println!("rsh: a fish-like shell with bash compatibility\nBuiltins: cd, exit, export, unset, echo, printf, pwd, alias, type, source,\n  eval, read, test, set, local, shift, jobs, fg, bg, history, pushd, popd,\n  dirs, trap, command, builtin, help"); 0 }
        "history" => builtin_history(state),
        "pushd" => builtin_pushd(args, state),
        "popd" => builtin_popd(state),
        "dirs" => builtin_dirs(state),
        "trap" => builtin_trap(args, state),
        "jobs" => { state.jobs.print_jobs(); 0 }
        "fg" => {
            let id = args.first().and_then(|s| s.trim_start_matches('%').parse().ok());
            match id {
                Some(id) => state.jobs.continue_fg(id),
                None => match state.jobs.get_last() {
                    Some(job) => { let id = job.id; state.jobs.continue_fg(id) }
                    None => { eprintln!("rsh: fg: no current job"); 1 }
                }
            }
        }
        "bg" => {
            let id = args.first().and_then(|s| s.trim_start_matches('%').parse().ok());
            match id {
                Some(id) => state.jobs.continue_bg(id),
                None => match state.jobs.get_last_stopped() {
                    Some(job) => { let id = job.id; state.jobs.continue_bg(id) }
                    None => { eprintln!("rsh: bg: no current job"); 1 }
                }
            }
        }
        "[[" => builtin_double_bracket(args),
        "command" | "builtin" => {
            if args.is_empty() { return 0; }
            // Run arg as command, bypassing aliases/functions
            let cmd = args.join(" ");
            match parser::parse(&cmd) {
                Ok(cmds) => {
                    let mut last = 0;
                    for c in &cmds { last = crate::executor::execute_complete_command(c, state); }
                    last
                }
                Err(_) => 1,
            }
        }
        _ => { eprintln!("rsh: {}: builtin not yet implemented", name); 1 }
    }
}

fn builtin_cd(args: &[String], state: &mut ShellState) -> i32 {
    let target = if args.is_empty() {
        state.home_dir.to_string_lossy().to_string()
    } else if args[0] == "-" {
        match state.get_var("OLDPWD") {
            Some(d) => {
                println!("{}", d);
                d.to_string()
            }
            None => { eprintln!("rsh: cd: OLDPWD not set"); return 1; }
        }
    } else {
        args[0].clone()
    };

    let old = env::current_dir().ok().map(|p| p.to_string_lossy().to_string());

    match env::set_current_dir(&target) {
        Ok(()) => {
            if let Ok(new_dir) = env::current_dir() {
                let new_str = new_dir.to_string_lossy().to_string();
                state.export_var("PWD", &new_str);
            }
            if let Some(old) = old {
                state.export_var("OLDPWD", &old);
            }
            0
        }
        Err(e) => {
            eprintln!("rsh: cd: {}: {}", target, e);
            1
        }
    }
}

fn builtin_exit(args: &[String]) -> i32 {
    let code = args.first()
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    std::process::exit(code);
}

fn builtin_export(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() {
        // Print all exported variables
        for (k, v) in &state.env_vars {
            println!("declare -x {}=\"{}\"", k, v);
        }
        return 0;
    }

    for arg in args {
        if let Some(eq_pos) = arg.find('=') {
            let name = &arg[..eq_pos];
            let value = &arg[eq_pos + 1..];
            state.export_var(name, value);
        } else {
            // Export existing variable
            if let Some(val) = state.local_vars.get(arg).cloned() {
                state.export_var(arg, &val);
            } else if !state.env_vars.contains_key(arg) {
                state.export_var(arg, "");
            }
        }
    }
    0
}

fn builtin_unset(args: &[String], state: &mut ShellState) -> i32 {
    for name in args {
        state.unset_var(name);
    }
    0
}

fn builtin_echo(args: &[String]) -> i32 {
    let mut newline = true;
    let mut interpret_escapes = false;
    let mut start = 0;

    // Parse flags
    for (i, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "-n" => { newline = false; start = i + 1; }
            "-e" => { interpret_escapes = true; start = i + 1; }
            "-E" => { interpret_escapes = false; start = i + 1; }
            "-ne" | "-en" => { newline = false; interpret_escapes = true; start = i + 1; }
            _ => break,
        }
    }

    let text = args[start..].join(" ");
    if interpret_escapes {
        print!("{}", unescape_echo(&text));
    } else {
        print!("{}", text);
    }
    if newline { println!(); }
    0
}

fn unescape_echo(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some('a') => result.push('\x07'),
                Some('b') => result.push('\x08'),
                Some('0') => {
                    result.push('\0');
                }
                Some(c2) => { result.push('\\'); result.push(c2); }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn builtin_pwd() -> i32 {
    match env::current_dir() {
        Ok(p) => { println!("{}", p.display()); 0 }
        Err(e) => { eprintln!("rsh: pwd: {}", e); 1 }
    }
}

fn builtin_alias(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() {
        for (k, v) in &state.aliases {
            println!("alias {}='{}'", k, v);
        }
        return 0;
    }
    for arg in args {
        if let Some(eq_pos) = arg.find('=') {
            let name = &arg[..eq_pos];
            let value = &arg[eq_pos + 1..];
            // Strip surrounding quotes if present
            let value = value.trim_matches('\'').trim_matches('"');
            state.aliases.insert(name.to_string(), value.to_string());
        } else {
            match state.aliases.get(arg) {
                Some(v) => println!("alias {}='{}'", arg, v),
                None => { eprintln!("rsh: alias: {}: not found", arg); return 1; }
            }
        }
    }
    0
}

fn builtin_unalias(args: &[String], state: &mut ShellState) -> i32 {
    for name in args {
        if name == "-a" {
            state.aliases.clear();
            return 0;
        }
        state.aliases.remove(name);
    }
    0
}

fn builtin_type(args: &[String], state: &mut ShellState) -> i32 {
    let mut ret = 0;
    for arg in args {
        if is_builtin(arg) {
            println!("{} is a shell builtin", arg);
        } else if state.aliases.contains_key(arg) {
            println!("{} is aliased to '{}'", arg, state.aliases[arg]);
        } else if state.functions.contains_key(arg) {
            println!("{} is a function", arg);
        } else if let Some(path) = find_in_path(arg) {
            println!("{} is {}", arg, path);
        } else {
            eprintln!("rsh: type: {}: not found", arg);
            ret = 1;
        }
    }
    ret
}

fn find_in_path(cmd: &str) -> Option<String> {
    if let Ok(path) = env::var("PATH") {
        for dir in path.split(':') {
            let full = format!("{}/{}", dir, cmd);
            if Path::new(&full).is_file() {
                return Some(full);
            }
        }
    }
    None
}

fn builtin_source(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() {
        eprintln!("rsh: source: filename argument required");
        return 1;
    }
    let path = &args[0];
    match std::fs::read_to_string(path) {
        Ok(content) => {
            match parser::parse(&content) {
                Ok(commands) => {
                    let mut last = 0;
                    for cmd in &commands {
                        last = crate::executor::execute_complete_command(cmd, state);
                    }
                    last
                }
                Err(e) => {
                    eprintln!("rsh: source: {}: {}", path, e);
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("rsh: source: {}: {}", path, e);
            1
        }
    }
}

fn builtin_eval(args: &[String], state: &mut ShellState) -> i32 {
    let input = args.join(" ");
    match parser::parse(&input) {
        Ok(commands) => {
            let mut last = 0;
            for cmd in &commands {
                last = crate::executor::execute_complete_command(cmd, state);
            }
            last
        }
        Err(e) => {
            eprintln!("rsh: eval: {}", e);
            1
        }
    }
}

fn builtin_read(args: &[String], state: &mut ShellState) -> i32 {
    let var_name = args.first().map(|s| s.as_str()).unwrap_or("REPLY");
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => 1, // EOF
        Ok(_) => {
            let line = line.trim_end_matches('\n').trim_end_matches('\r');
            state.set_var(var_name, line);
            0
        }
        Err(_) => 1,
    }
}

fn builtin_test(args: &[String]) -> i32 {
    // Filter out trailing ] if invoked as [
    let args: Vec<&str> = args.iter()
        .map(|s| s.as_str())
        .filter(|s| *s != "]")
        .collect();

    if args.is_empty() { return 1; }

    match args.len() {
        1 => {
            // [ string ] - true if non-empty
            if args[0].is_empty() { 1 } else { 0 }
        }
        2 => {
            match args[0] {
                "-n" => if !args[1].is_empty() { 0 } else { 1 },
                "-z" => if args[1].is_empty() { 0 } else { 1 },
                "-f" => if Path::new(args[1]).is_file() { 0 } else { 1 },
                "-d" => if Path::new(args[1]).is_dir() { 0 } else { 1 },
                "-e" => if Path::new(args[1]).exists() { 0 } else { 1 },
                "-r" => if Path::new(args[1]).exists() { 0 } else { 1 }, // simplified
                "-w" => if Path::new(args[1]).exists() { 0 } else { 1 }, // simplified
                "-x" => if Path::new(args[1]).exists() { 0 } else { 1 }, // simplified
                "-s" => {
                    if let Ok(m) = std::fs::metadata(args[1]) {
                        if m.len() > 0 { 0 } else { 1 }
                    } else { 1 }
                }
                "!" => builtin_test(&[args[1].to_string()]) ^ 1,
                _ => 1,
            }
        }
        3 => {
            match args[1] {
                "=" | "==" => if args[0] == args[2] { 0 } else { 1 },
                "!=" => if args[0] != args[2] { 0 } else { 1 },
                "-eq" => cmp_int(args[0], args[2], |a, b| a == b),
                "-ne" => cmp_int(args[0], args[2], |a, b| a != b),
                "-lt" => cmp_int(args[0], args[2], |a, b| a < b),
                "-le" => cmp_int(args[0], args[2], |a, b| a <= b),
                "-gt" => cmp_int(args[0], args[2], |a, b| a > b),
                "-ge" => cmp_int(args[0], args[2], |a, b| a >= b),
                _ => 1,
            }
        }
        _ => 1,
    }
}

fn cmp_int(a: &str, b: &str, f: fn(i64, i64) -> bool) -> i32 {
    match (a.parse::<i64>(), b.parse::<i64>()) {
        (Ok(a), Ok(b)) => if f(a, b) { 0 } else { 1 },
        _ => 2,
    }
}

fn builtin_set(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() {
        let mut all: Vec<_> = state.env_vars.iter().chain(state.local_vars.iter()).collect();
        all.sort_by_key(|(k, _)| (*k).clone());
        for (k, v) in all { println!("{}={}", k, v); }
        return 0;
    }
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-e" => state.shell_opts.errexit = true,
            "+e" => state.shell_opts.errexit = false,
            "-x" => state.shell_opts.xtrace = true,
            "+x" => state.shell_opts.xtrace = false,
            "-o" => {
                i += 1;
                if i < args.len() {
                    match args[i].as_str() {
                        "errexit" => state.shell_opts.errexit = true,
                        "xtrace" => state.shell_opts.xtrace = true,
                        "pipefail" => state.shell_opts.pipefail = true,
                        _ => { eprintln!("rsh: set: unknown option: {}", args[i]); return 1; }
                    }
                }
            }
            "+o" => {
                i += 1;
                if i < args.len() {
                    match args[i].as_str() {
                        "errexit" => state.shell_opts.errexit = false,
                        "xtrace" => state.shell_opts.xtrace = false,
                        "pipefail" => state.shell_opts.pipefail = false,
                        _ => { eprintln!("rsh: set: unknown option: {}", args[i]); return 1; }
                    }
                }
            }
            "--" => {
                state.positional_params = args[i + 1..].to_vec();
                return 0;
            }
            _ => {
                // Set positional params
                state.positional_params = args[i..].to_vec();
                return 0;
            }
        }
        i += 1;
    }
    0
}

fn builtin_local(args: &[String], state: &mut ShellState) -> i32 {
    for arg in args {
        if let Some(eq_pos) = arg.find('=') {
            let name = &arg[..eq_pos];
            let value = &arg[eq_pos + 1..];
            state.local_vars.insert(name.to_string(), value.to_string());
        } else {
            state.local_vars.insert(arg.clone(), String::new());
        }
    }
    0
}

fn builtin_history(_state: &ShellState) -> i32 {
    // Read history file directly
    let path = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp")).join(".rsh_history");
    if let Ok(content) = std::fs::read_to_string(&path) {
        for (i, line) in content.lines().enumerate() {
            println!("{:5}  {}", i + 1, line);
        }
    }
    0
}

fn builtin_printf(args: &[String]) -> i32 {
    if args.is_empty() { return 0; }
    let fmt = &args[0];
    let params = &args[1..];
    let mut pi = 0;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => print!("\n"),
                Some('t') => print!("\t"),
                Some('r') => print!("\r"),
                Some('\\') => print!("\\"),
                Some('0') => print!("\0"),
                Some('a') => print!("\x07"),
                Some('b') => print!("\x08"),
                Some(c2) => { print!("\\{}", c2); }
                None => print!("\\"),
            }
        } else if c == '%' {
            let arg = params.get(pi).map(|s| s.as_str()).unwrap_or("");
            match chars.next() {
                Some('s') => print!("{}", arg),
                Some('d') | Some('i') => print!("{}", arg.parse::<i64>().unwrap_or(0)),
                Some('f') => print!("{}", arg.parse::<f64>().unwrap_or(0.0)),
                Some('x') => print!("{:x}", arg.parse::<i64>().unwrap_or(0)),
                Some('X') => print!("{:X}", arg.parse::<i64>().unwrap_or(0)),
                Some('o') => print!("{:o}", arg.parse::<i64>().unwrap_or(0)),
                Some('c') => print!("{}", arg.chars().next().unwrap_or('\0')),
                Some('%') => { print!("%"); continue; } // no param consumed
                Some(c2) => print!("%{}", c2),
                None => print!("%"),
            }
            pi += 1;
        } else {
            print!("{}", c);
        }
    }
    use std::io::Write;
    std::io::stdout().flush().ok();
    0
}

fn builtin_shift(state: &mut ShellState) -> i32 {
    if !state.positional_params.is_empty() {
        state.positional_params.remove(0);
    }
    0
}

fn builtin_pushd(args: &[String], state: &mut ShellState) -> i32 {
    let cwd = env::current_dir().ok();
    let target = if args.is_empty() {
        // Swap top two
        match state.dir_stack.pop() {
            Some(d) => d.to_string_lossy().to_string(),
            None => { eprintln!("rsh: pushd: no other directory"); return 1; }
        }
    } else {
        args[0].clone()
    };
    if let Some(cwd) = cwd {
        state.dir_stack.push(cwd);
    }
    match env::set_current_dir(&target) {
        Ok(()) => {
            if let Ok(new_dir) = env::current_dir() {
                state.export_var("PWD", &new_dir.to_string_lossy());
            }
            builtin_dirs(state);
            0
        }
        Err(e) => { eprintln!("rsh: pushd: {}: {}", target, e); 1 }
    }
}

fn builtin_popd(state: &mut ShellState) -> i32 {
    match state.dir_stack.pop() {
        Some(dir) => {
            match env::set_current_dir(&dir) {
                Ok(()) => {
                    state.export_var("PWD", &dir.to_string_lossy());
                    builtin_dirs(state);
                    0
                }
                Err(e) => { eprintln!("rsh: popd: {}", e); 1 }
            }
        }
        None => { eprintln!("rsh: popd: directory stack empty"); 1 }
    }
}

fn builtin_dirs(state: &ShellState) -> i32 {
    let cwd = env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    print!("{}", cwd);
    for d in state.dir_stack.iter().rev() {
        print!(" {}", d.display());
    }
    println!();
    0
}

fn builtin_double_bracket(args: &[String]) -> i32 {
    // [[ expr ]] - extended test. Args are everything between [[ and ]]
    let args: Vec<&str> = args.iter()
        .map(|s| s.as_str())
        .filter(|s| *s != "]]")
        .collect();
    if args.is_empty() { return 1; }
    eval_cond_expr(&args, &mut 0)
}

fn eval_cond_expr(args: &[&str], pos: &mut usize) -> i32 {
    eval_cond_or(args, pos)
}

fn eval_cond_or(args: &[&str], pos: &mut usize) -> i32 {
    let mut left = eval_cond_and(args, pos);
    while *pos < args.len() && args[*pos] == "||" {
        *pos += 1;
        let right = eval_cond_and(args, pos);
        left = if left == 0 || right == 0 { 0 } else { 1 };
    }
    left
}

fn eval_cond_and(args: &[&str], pos: &mut usize) -> i32 {
    let mut left = eval_cond_primary(args, pos);
    while *pos < args.len() && args[*pos] == "&&" {
        *pos += 1;
        let right = eval_cond_primary(args, pos);
        left = if left == 0 && right == 0 { 0 } else { 1 };
    }
    left
}

fn eval_cond_primary(args: &[&str], pos: &mut usize) -> i32 {
    if *pos >= args.len() { return 1; }

    // Negation
    if args[*pos] == "!" {
        *pos += 1;
        return eval_cond_primary(args, pos) ^ 1;
    }

    // Parenthesized expression
    if args[*pos] == "(" {
        *pos += 1;
        let r = eval_cond_expr(args, pos);
        if *pos < args.len() && args[*pos] == ")" { *pos += 1; }
        return r;
    }

    // Unary operators
    if args[*pos].starts_with('-') && args[*pos].len() == 2 && *pos + 1 < args.len() {
        let op = args[*pos];
        let operand = args[*pos + 1];
        let result = match op {
            "-n" => { *pos += 2; if !operand.is_empty() { 0 } else { 1 } }
            "-z" => { *pos += 2; if operand.is_empty() { 0 } else { 1 } }
            "-f" => { *pos += 2; if Path::new(operand).is_file() { 0 } else { 1 } }
            "-d" => { *pos += 2; if Path::new(operand).is_dir() { 0 } else { 1 } }
            "-e" => { *pos += 2; if Path::new(operand).exists() { 0 } else { 1 } }
            "-s" => { *pos += 2; std::fs::metadata(operand).map(|m| if m.len() > 0 { 0 } else { 1 }).unwrap_or(1) }
            "-r" | "-w" | "-x" => { *pos += 2; if Path::new(operand).exists() { 0 } else { 1 } }
            _ => {
                // Check if next token is a binary operator
                if *pos + 2 < args.len() {
                    return eval_cond_binary(args, pos);
                }
                *pos += 2;
                1
            }
        };
        return result;
    }

    // Binary expression or standalone string test
    if *pos + 1 < args.len() && is_cond_binary_op(args[*pos + 1]) {
        return eval_cond_binary(args, pos);
    }

    // Standalone string (non-empty = true)
    let s = args[*pos];
    *pos += 1;
    if s.is_empty() { 1 } else { 0 }
}

fn is_cond_binary_op(op: &str) -> bool {
    matches!(op, "==" | "=" | "!=" | "<" | ">" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" | "=~")
}

fn eval_cond_binary(args: &[&str], pos: &mut usize) -> i32 {
    if *pos + 2 > args.len() { return 1; }
    let left = args[*pos];
    let op = args[*pos + 1];
    let right = args[*pos + 2];
    *pos += 3;
    match op {
        "==" | "=" => {
            // Pattern matching (glob)
            if right.contains('*') || right.contains('?') {
                let matched = glob_match(right, left);
                if matched { 0 } else { 1 }
            } else {
                if left == right { 0 } else { 1 }
            }
        }
        "!=" => {
            if right.contains('*') || right.contains('?') {
                if glob_match(right, left) { 1 } else { 0 }
            } else {
                if left != right { 0 } else { 1 }
            }
        }
        "<" => if left < right { 0 } else { 1 },
        ">" => if left > right { 0 } else { 1 },
        "-eq" => cmp_int(left, right, |a, b| a == b),
        "-ne" => cmp_int(left, right, |a, b| a != b),
        "-lt" => cmp_int(left, right, |a, b| a < b),
        "-le" => cmp_int(left, right, |a, b| a <= b),
        "-gt" => cmp_int(left, right, |a, b| a > b),
        "-ge" => cmp_int(left, right, |a, b| a >= b),
        "=~" => {
            // Regex match
            // Simple implementation: treat as glob for now
            if glob_match(right, left) { 0 } else { 1 }
        }
        _ => 1,
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_rec(&p, 0, &t, 0)
}

fn glob_match_rec(pat: &[char], pi: usize, text: &[char], ti: usize) -> bool {
    if pi >= pat.len() { return ti >= text.len(); }
    if pat[pi] == '*' {
        for i in ti..=text.len() {
            if glob_match_rec(pat, pi + 1, text, i) { return true; }
        }
        return false;
    }
    if ti >= text.len() { return false; }
    if pat[pi] == '?' || pat[pi] == text[ti] {
        return glob_match_rec(pat, pi + 1, text, ti + 1);
    }
    false
}

fn builtin_trap(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() {
        for (sig, cmd) in &state.traps {
            println!("trap -- '{}' {}", cmd, sig);
        }
        return 0;
    }
    if args.len() == 1 && args[0] == "-l" {
        println!("EXIT HUP INT QUIT TERM USR1 USR2 ALRM");
        return 0;
    }
    if args.len() >= 2 {
        let action = &args[0];
        for sig in &args[1..] {
            if action == "-" || action.is_empty() {
                state.traps.remove(sig.as_str());
            } else {
                state.traps.insert(sig.clone(), action.clone());
            }
        }
    }
    0
}
