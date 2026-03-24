/// Built-in shell commands.

use crate::environment::ShellState;
use crate::parser;
use std::env;
use std::path::Path;

pub fn is_builtin(name: &str) -> bool {
    matches!(name,
        "cd" | "exit" | "export" | "unset" | "echo" | "pwd" | "alias" | "unalias" |
        "type" | "source" | "." | "eval" | "read" | "true" | "false" | "test" | "[" |
        "return" | "break" | "continue" | "shift" | "set" | "local" |
        "jobs" | "fg" | "bg" | "history" | "help"
    )
}

pub fn run_builtin(name: &str, args: &[String], state: &mut ShellState) -> i32 {
    match name {
        "cd" => builtin_cd(args, state),
        "exit" => builtin_exit(args),
        "export" => builtin_export(args, state),
        "unset" => builtin_unset(args, state),
        "echo" => builtin_echo(args),
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
        "shift" => 0,
        "help" => { eprintln!("rsh: a fish-like shell with bash compatibility"); 0 }
        "history" => builtin_history(state),
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
        // Print all variables
        for (k, v) in &state.env_vars {
            println!("{}={}", k, v);
        }
        for (k, v) in &state.local_vars {
            println!("{}={}", k, v);
        }
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
    // Will be implemented with history module
    println!("(history not yet loaded)");
    0
}
