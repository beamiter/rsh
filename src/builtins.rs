/// Built-in shell commands.

use crate::environment::ShellState;
use crate::parser;
use std::env;
use std::path::Path;

pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "export", "unset", "echo", "printf", "pwd",
    "alias", "unalias", "type", "source", ".", "eval", "read",
    "true", "false", "test", "[", "return", "break", "continue",
    "shift", "set", "local", "jobs", "fg", "bg", "history", "help",
    "pushd", "popd", "dirs", "trap", "command", "builtin", "[[",
    "declare", "z", "hook", "complete", "compgen", "disown", "shopt",
    "from-json", "to-json", "to-table", "where", "sort-by", "select",
    "bookmark", "from-csv", "group-by", "unique", "count", "math",
];

pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
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
        "return" | "break" | "continue" => 0,
        "shift" => builtin_shift(state),
        "help" => { println!("rsh: a fish-like shell with bash compatibility\nBuiltins: cd, exit, export, unset, echo, printf, pwd, alias, type, source,\n  eval, read, test, set, local, shift, jobs, fg, bg, history, pushd, popd,\n  dirs, trap, command, builtin, declare, z, bookmark, hook, complete, compgen,\n  disown, from-json, to-json, to-table, where, sort-by, select, help"); 0 }
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
        "[[" => builtin_double_bracket(args, state),
        "command" => {
            if args.is_empty() { return 0; }
            let cmd_name = &args[0];
            if is_builtin(cmd_name) {
                run_builtin(cmd_name, &args[1..], state)
            } else {
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
        }
        "builtin" => {
            if args.is_empty() { return 0; }
            let cmd_name = &args[0];
            if is_builtin(cmd_name) {
                run_builtin(cmd_name, &args[1..], state)
            } else {
                eprintln!("rsh: builtin: {}: not a shell builtin", cmd_name);
                1
            }
        }
        // New builtins
        "declare" => builtin_declare(args, state),
        "z" => builtin_z(args, state),
        "hook" => builtin_hook(args, state),
        "complete" => builtin_complete(args, state),
        "compgen" => builtin_compgen(args, state),
        "disown" => builtin_disown(args, state),
        "shopt" => builtin_shopt(args, state),
        "from-json" => builtin_from_json(),
        "to-json" => builtin_to_json(),
        "to-table" => builtin_to_table(),
        "where" => builtin_where(args),
        "sort-by" => builtin_sort_by(args),
        "select" => builtin_select(args),
        "bookmark" => builtin_bookmark(args, state),
        "from-csv" => builtin_from_csv(),
        "group-by" => builtin_group_by(args),
        "unique" => builtin_unique(args),
        "count" => builtin_count(),
        "math" => builtin_math(args),
        _ => { eprintln!("rsh: {}: builtin not yet implemented", name); 1 }
    }
}

// ============================================================
// Original builtins
// ============================================================

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
            // z-jump: record directory visit
            if let Ok(cwd) = env::current_dir() {
                if let Ok(mut z_db) = crate::zjump::get_z_db().lock() {
                    z_db.add(&cwd.to_string_lossy());
                }
            }
            // chpwd hooks
            let hooks = state.hooks.chpwd.clone();
            crate::hooks::run_hooks(&hooks, state);
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
        let mut vars: Vec<_> = state.env_vars.iter().collect();
        vars.sort_by_key(|(k, _)| (*k).clone());
        for (k, v) in vars {
            println!("declare -x {}=\"{}\"", k, v);
        }
        return 0;
    }

    if args.first().map(|s| s.as_str()) == Some("-n") {
        for arg in &args[1..] {
            if let Some(val) = state.env_vars.remove(arg) {
                env::remove_var(arg);
                state.local_vars.insert(arg.clone(), val);
            }
        }
        return 0;
    }

    for arg in args {
        if let Some(eq_pos) = arg.find('=') {
            let name = &arg[..eq_pos];
            let value = &arg[eq_pos + 1..];
            state.export_var(name, value);
        } else {
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
        if name == "-v" || name == "-f" { continue; }
        // Support unset arr[idx]
        if let Some(bracket) = name.find('[') {
            if name.ends_with(']') {
                let var_name = &name[..bracket];
                let idx = &name[bracket + 1..name.len() - 1];
                if let Some(arr) = state.arrays.get_mut(var_name) {
                    if let Ok(i) = idx.parse::<usize>() {
                        if i < arr.len() {
                            arr[i] = String::new();
                        }
                    }
                } else if let Some(map) = state.assoc_arrays.get_mut(var_name) {
                    map.remove(idx);
                }
                continue;
            }
        }
        state.unset_var(name);
    }
    0
}

fn builtin_echo(args: &[String]) -> i32 {
    let mut newline = true;
    let mut interpret_escapes = false;
    let mut start = 0;

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
                Some('0') => result.push('\0'),
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

    // Try to find the file
    let resolved_path = if Path::new(path).is_file() {
        // File exists at given path
        path.to_string()
    } else if !path.contains('/') {
        // No slashes in path, try $PATH search
        match find_in_path(path) {
            Some(found) => found,
            None => {
                eprintln!("rsh: source: {}: No such file or directory", path);
                return 1;
            }
        }
    } else {
        // Absolute or relative path doesn't exist
        eprintln!("rsh: source: {}: No such file or directory", path);
        return 1;
    };

    match std::fs::read_to_string(&resolved_path) {
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
                    eprintln!("rsh: source: {}: parse error: {}", resolved_path, e);
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("rsh: source: {}: {}", resolved_path, e);
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
    let mut prompt_str = None;
    let mut silent = false;
    let mut raw = false;
    let mut var_names: Vec<&str> = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-p" => {
                i += 1;
                if i < args.len() { prompt_str = Some(args[i].as_str()); }
            }
            "-s" => silent = true,
            "-r" => raw = true,
            s if s.starts_with('-') => {}
            _ => { var_names.push(&args[i]); }
        }
        i += 1;
    }
    if var_names.is_empty() {
        var_names.push("REPLY");
    }

    if let Some(p) = prompt_str {
        eprint!("{}", p);
        use std::io::Write;
        std::io::stderr().flush().ok();
    }

    if silent {
        std::process::Command::new("stty").arg("-echo").status().ok();
    }

    let mut line = String::new();
    let result = match std::io::stdin().read_line(&mut line) {
        Ok(0) => 1,
        Ok(_) => {
            let line = line.trim_end_matches('\n').trim_end_matches('\r');
            let line = if !raw {
                line.replace("\\\n", "")
            } else {
                line.to_string()
            };

            if var_names.len() == 1 {
                state.set_var(var_names[0], &line);
            } else {
                let parts: Vec<&str> = line.splitn(var_names.len(), char::is_whitespace).collect();
                for (vi, var) in var_names.iter().enumerate() {
                    state.set_var(var, parts.get(vi).unwrap_or(&""));
                }
            }
            0
        }
        Err(_) => 1,
    };

    if silent {
        std::process::Command::new("stty").arg("echo").status().ok();
        eprintln!();
    }

    result
}

fn builtin_test(args: &[String]) -> i32 {
    let args: Vec<&str> = args.iter()
        .map(|s| s.as_str())
        .filter(|s| *s != "]")
        .collect();

    if args.is_empty() { return 1; }

    match args.len() {
        1 => {
            if args[0].is_empty() { 1 } else { 0 }
        }
        2 => {
            match args[0] {
                "-n" => if !args[1].is_empty() { 0 } else { 1 },
                "-z" => if args[1].is_empty() { 0 } else { 1 },
                "-f" => if Path::new(args[1]).is_file() { 0 } else { 1 },
                "-d" => if Path::new(args[1]).is_dir() { 0 } else { 1 },
                "-e" => if Path::new(args[1]).exists() { 0 } else { 1 },
                "-r" => if Path::new(args[1]).exists() { 0 } else { 1 },
                "-w" => if Path::new(args[1]).exists() { 0 } else { 1 },
                "-x" => if Path::new(args[1]).exists() { 0 } else { 1 },
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
                        "globstar" => state.shell_opts.globstar = true,
                        "vi" => state.editing_mode = crate::environment::EditingMode::Vi,
                        "emacs" => state.editing_mode = crate::environment::EditingMode::Emacs,
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
                        "globstar" => state.shell_opts.globstar = false,
                        "vi" => state.editing_mode = crate::environment::EditingMode::Emacs,
                        "emacs" => state.editing_mode = crate::environment::EditingMode::Vi,
                        _ => { eprintln!("rsh: set: unknown option: {}", args[i]); return 1; }
                    }
                }
            }
            "--" => {
                state.positional_params = args[i + 1..].to_vec();
                return 0;
            }
            _ => {
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
                Some('%') => { print!("%"); continue; }
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

// ============================================================
// [[ ]] with real regex support (Phase 2)
// ============================================================

fn builtin_double_bracket(args: &[String], state: &mut ShellState) -> i32 {
    let args: Vec<&str> = args.iter()
        .map(|s| s.as_str())
        .filter(|s| *s != "]]")
        .collect();
    if args.is_empty() { return 1; }
    eval_cond_expr(&args, &mut 0, state)
}

fn eval_cond_expr(args: &[&str], pos: &mut usize, state: &mut ShellState) -> i32 {
    eval_cond_or(args, pos, state)
}

fn eval_cond_or(args: &[&str], pos: &mut usize, state: &mut ShellState) -> i32 {
    let mut left = eval_cond_and(args, pos, state);
    while *pos < args.len() && args[*pos] == "||" {
        *pos += 1;
        let right = eval_cond_and(args, pos, state);
        left = if left == 0 || right == 0 { 0 } else { 1 };
    }
    left
}

fn eval_cond_and(args: &[&str], pos: &mut usize, state: &mut ShellState) -> i32 {
    let mut left = eval_cond_primary(args, pos, state);
    while *pos < args.len() && args[*pos] == "&&" {
        *pos += 1;
        let right = eval_cond_primary(args, pos, state);
        left = if left == 0 && right == 0 { 0 } else { 1 };
    }
    left
}

fn eval_cond_primary(args: &[&str], pos: &mut usize, state: &mut ShellState) -> i32 {
    if *pos >= args.len() { return 1; }

    if args[*pos] == "!" {
        *pos += 1;
        return eval_cond_primary(args, pos, state) ^ 1;
    }

    if args[*pos] == "(" {
        *pos += 1;
        let r = eval_cond_expr(args, pos, state);
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
                if *pos + 2 < args.len() {
                    return eval_cond_binary(args, pos, state);
                }
                *pos += 2;
                1
            }
        };
        return result;
    }

    // Binary expression or standalone string test
    if *pos + 1 < args.len() && is_cond_binary_op(args[*pos + 1]) {
        return eval_cond_binary(args, pos, state);
    }

    let s = args[*pos];
    *pos += 1;
    if s.is_empty() { 1 } else { 0 }
}

fn is_cond_binary_op(op: &str) -> bool {
    matches!(op, "==" | "=" | "!=" | "<" | ">" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" | "=~")
}

fn eval_cond_binary(args: &[&str], pos: &mut usize, state: &mut ShellState) -> i32 {
    if *pos + 2 > args.len() { return 1; }
    let left = args[*pos];
    let op = args[*pos + 1];
    let right = args[*pos + 2];
    *pos += 3;
    match op {
        "==" | "=" => {
            if right.contains('*') || right.contains('?') {
                if glob_match(right, left) { 0 } else { 1 }
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
            // Real regex matching with BASH_REMATCH
            match regex::Regex::new(right) {
                Ok(re) => {
                    if let Some(captures) = re.captures(left) {
                        // Store BASH_REMATCH array
                        let mut rematch = Vec::new();
                        for i in 0..captures.len() {
                            rematch.push(captures.get(i).map(|m| m.as_str().to_string()).unwrap_or_default());
                        }
                        state.arrays.insert("BASH_REMATCH".to_string(), rematch);
                        0
                    } else {
                        state.arrays.insert("BASH_REMATCH".to_string(), Vec::new());
                        1
                    }
                }
                Err(_) => 2,
            }
        }
        _ => 1,
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    crate::glob_match::glob_match(pattern, text)
}

// ============================================================
// declare (Phase 1)
// ============================================================

fn builtin_declare(args: &[String], state: &mut ShellState) -> i32 {
    let mut indexed = false;
    let mut associative = false;
    let mut print = false;
    let mut names: Vec<&str> = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-a" => indexed = true,
            "-A" => associative = true,
            "-p" => print = true,
            s => names.push(s),
        }
        i += 1;
    }

    if print {
        for name in &names {
            if let Some(arr) = state.arrays.get(*name) {
                println!("declare -a {}=({})", name, arr.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(" "));
            } else if let Some(map) = state.assoc_arrays.get(*name) {
                let pairs: Vec<String> = map.iter().map(|(k, v)| format!("[{}]=\"{}\"", k, v)).collect();
                println!("declare -A {}=({})", name, pairs.join(" "));
            } else if let Some(val) = state.get_var(name) {
                println!("declare -- {}=\"{}\"", name, val);
            }
        }
        return 0;
    }

    for name in &names {
        // Handle name=value
        let (var_name, value) = if let Some(eq) = name.find('=') {
            (&name[..eq], Some(&name[eq + 1..]))
        } else {
            (*name, None)
        };

        if associative {
            if !state.assoc_arrays.contains_key(var_name) {
                state.assoc_arrays.insert(var_name.to_string(), std::collections::HashMap::new());
            }
        } else if indexed {
            if !state.arrays.contains_key(var_name) {
                state.arrays.insert(var_name.to_string(), Vec::new());
            }
        }

        if let Some(val) = value {
            if !indexed && !associative {
                state.set_var(var_name, val);
            }
        }
    }
    0
}

// ============================================================
// z-jump (Phase 5)
// ============================================================

fn builtin_z(args: &[String], state: &mut ShellState) -> i32 {
    let z_db = crate::zjump::get_z_db();
    let mut z_db = z_db.lock().unwrap();

    if args.is_empty() || (args.len() == 1 && args[0] == "-l") {
        for (path, score) in z_db.list() {
            println!("{:>10.1}  {}", score, path);
        }
        return 0;
    }

    if args.len() == 2 && args[0] == "-x" {
        z_db.remove(&args[1]);
        return 0;
    }

    if args.len() == 1 && args[0] == "-c" {
        if let Ok(cwd) = env::current_dir() {
            z_db.remove(&cwd.to_string_lossy());
        }
        return 0;
    }

    let keywords: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match z_db.query(&keywords) {
        Some(target) => {
            // Do cd
            let old = env::current_dir().ok().map(|p| p.to_string_lossy().to_string());
            match env::set_current_dir(&target) {
                Ok(()) => {
                    println!("{}", target);
                    if let Ok(new_dir) = env::current_dir() {
                        state.export_var("PWD", &new_dir.to_string_lossy());
                    }
                    if let Some(old) = old {
                        state.export_var("OLDPWD", &old);
                    }
                    let hooks = state.hooks.chpwd.clone();
                    crate::hooks::run_hooks(&hooks, state);
                    0
                }
                Err(e) => {
                    eprintln!("rsh: z: {}: {}", target, e);
                    1
                }
            }
        }
        None => {
            eprintln!("rsh: z: no match for: {}", args.join(" "));
            1
        }
    }
}

// ============================================================
// hook (Phase 4)
// ============================================================

fn builtin_hook(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() || args[0] == "list" {
        println!("precmd:  {:?}", state.hooks.precmd);
        println!("preexec: {:?}", state.hooks.preexec);
        println!("chpwd:   {:?}", state.hooks.chpwd);
        return 0;
    }

    if args.len() < 3 {
        eprintln!("Usage: hook add|remove precmd|preexec|chpwd <function>");
        return 1;
    }

    let action = &args[0];
    let hook_type = &args[1];
    let func = &args[2];

    let hook_list = match hook_type.as_str() {
        "precmd" => &mut state.hooks.precmd,
        "preexec" => &mut state.hooks.preexec,
        "chpwd" => &mut state.hooks.chpwd,
        _ => {
            eprintln!("rsh: hook: unknown hook type: {}", hook_type);
            return 1;
        }
    };

    match action.as_str() {
        "add" => {
            if !hook_list.contains(func) {
                hook_list.push(func.clone());
            }
        }
        "remove" => {
            hook_list.retain(|h| h != func);
        }
        _ => {
            eprintln!("rsh: hook: unknown action: {} (use add or remove)", action);
            return 1;
        }
    }
    0
}

// ============================================================
// complete / compgen (Phase 7)
// ============================================================

fn builtin_complete(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() {
        // List all completion specs
        for (cmd, spec) in &state.completion_specs {
            let mut parts = Vec::new();
            if let Some(ref wl) = spec.word_list {
                parts.push(format!("-W \"{}\"", wl.join(" ")));
            }
            if let Some(ref f) = spec.function {
                parts.push(format!("-F {}", f));
            }
            if spec.directory { parts.push("-d".to_string()); }
            if spec.file { parts.push("-f".to_string()); }
            println!("complete {} {}", parts.join(" "), cmd);
        }
        return 0;
    }

    // Parse flags
    let mut word_list: Option<Vec<String>> = None;
    let mut function: Option<String> = None;
    let mut directory = false;
    let mut file = false;
    let mut glob_pattern: Option<String> = None;
    let mut filter_pattern: Option<String> = None;
    let mut prefix: Option<String> = None;
    let mut suffix: Option<String> = None;
    let mut remove = false;
    let mut command_name = String::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-W" => { i += 1; if i < args.len() { word_list = Some(args[i].split_whitespace().map(|s| s.to_string()).collect()); } }
            "-F" => { i += 1; if i < args.len() { function = Some(args[i].clone()); } }
            "-d" => directory = true,
            "-f" => file = true,
            "-G" => { i += 1; if i < args.len() { glob_pattern = Some(args[i].clone()); } }
            "-X" => { i += 1; if i < args.len() { filter_pattern = Some(args[i].clone()); } }
            "-P" => { i += 1; if i < args.len() { prefix = Some(args[i].clone()); } }
            "-S" => { i += 1; if i < args.len() { suffix = Some(args[i].clone()); } }
            "-r" => remove = true,
            _ => command_name = args[i].clone(),
        }
        i += 1;
    }

    if command_name.is_empty() {
        eprintln!("rsh: complete: no command specified");
        return 1;
    }

    if remove {
        state.completion_specs.remove(&command_name);
        return 0;
    }

    state.completion_specs.insert(command_name.clone(), crate::environment::CompletionSpec {
        command: command_name,
        word_list,
        function,
        directory,
        file,
        glob_pattern,
        filter_pattern,
        prefix,
        suffix,
    });
    0
}

fn builtin_compgen(args: &[String], _state: &mut ShellState) -> i32 {
    if args.is_empty() { return 0; }
    let mut word_list: Vec<String> = Vec::new();
    let mut prefix = "";
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-W" => { i += 1; if i < args.len() { word_list = args[i].split_whitespace().map(|s| s.to_string()).collect(); } }
            s if !s.starts_with('-') => { prefix = s; }
            _ => {}
        }
        i += 1;
    }

    for word in &word_list {
        if word.starts_with(prefix) {
            println!("{}", word);
        }
    }
    0
}

// ============================================================
// disown (Phase 8)
// ============================================================

fn builtin_disown(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() || args[0] == "-a" {
        // Disown all or last
        if args.first().map(|s| s.as_str()) == Some("-a") {
            state.jobs.jobs.clear();
        } else if let Some(job) = state.jobs.get_last() {
            let id = job.id;
            state.jobs.jobs.retain(|j| j.id != id);
        } else {
            eprintln!("rsh: disown: no current job");
            return 1;
        }
        return 0;
    }

    let id: Option<usize> = args[0].trim_start_matches('%').parse().ok();
    match id {
        Some(id) => {
            state.jobs.jobs.retain(|j| j.id != id);
            0
        }
        None => {
            eprintln!("rsh: disown: {}: no such job", args[0]);
            1
        }
    }
}

// ============================================================
// shopt (shell options)
// ============================================================

fn builtin_shopt(args: &[String], state: &mut ShellState) -> i32 {
    // shopt [-psuE] [optname ...]
    // -p: print all options (default when no args)
    // -s: set option
    // -u: unset option

    if args.is_empty() {
        // Print all options
        print_shopt_options(&state.shell_opts);
        return 0;
    }

    let mut setting = None;
    let mut print_all = false;
    let mut opts = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-s" => setting = Some(true),
            "-u" => setting = Some(false),
            "-p" => print_all = true,
            s if s.starts_with('-') => {
                // Unknown option, treat as option name (for compat)
                opts.push(s[1..].to_string());
            }
            _ => opts.push(args[i].clone()),
        }
        i += 1;
    }

    if print_all {
        print_shopt_options(&state.shell_opts);
        return 0;
    }

    if opts.is_empty() {
        // No options specified, print all
        print_shopt_options(&state.shell_opts);
        return 0;
    }

    let mut exit_code = 0;
    for opt in opts {
        match setting {
            Some(true) => {
                // Set option
                match opt.as_str() {
                    "dotglob" => state.shell_opts.dotglob = true,
                    "nullglob" => state.shell_opts.nullglob = true,
                    "failglob" => state.shell_opts.failglob = true,
                    "extglob" => state.shell_opts.extglob = true,
                    "nocaseglob" => state.shell_opts.nocaseglob = true,
                    "noglob" => state.shell_opts.noglob = true,
                    "lastpipe" => state.shell_opts.lastpipe = true,
                    "autocd" => state.shell_opts.autocd = true,
                    "cdspell" => state.shell_opts.cdspell = true,
                    "checkwinsize" => state.shell_opts.checkwinsize = true,
                    "inherit_errexit" => state.shell_opts.inherit_errexit = true,
                    _ => {
                        eprintln!("rsh: shopt: {}: invalid option name", opt);
                        exit_code = 1;
                    }
                }
            }
            Some(false) => {
                // Unset option
                match opt.as_str() {
                    "dotglob" => state.shell_opts.dotglob = false,
                    "nullglob" => state.shell_opts.nullglob = false,
                    "failglob" => state.shell_opts.failglob = false,
                    "extglob" => state.shell_opts.extglob = false,
                    "nocaseglob" => state.shell_opts.nocaseglob = false,
                    "noglob" => state.shell_opts.noglob = false,
                    "lastpipe" => state.shell_opts.lastpipe = false,
                    "autocd" => state.shell_opts.autocd = false,
                    "cdspell" => state.shell_opts.cdspell = false,
                    "checkwinsize" => state.shell_opts.checkwinsize = false,
                    "inherit_errexit" => state.shell_opts.inherit_errexit = false,
                    _ => {
                        eprintln!("rsh: shopt: {}: invalid option name", opt);
                        exit_code = 1;
                    }
                }
            }
            None => {
                // No -s or -u specified, just report status
                let value = match opt.as_str() {
                    "dotglob" => Some(state.shell_opts.dotglob),
                    "nullglob" => Some(state.shell_opts.nullglob),
                    "failglob" => Some(state.shell_opts.failglob),
                    "extglob" => Some(state.shell_opts.extglob),
                    "nocaseglob" => Some(state.shell_opts.nocaseglob),
                    "noglob" => Some(state.shell_opts.noglob),
                    "lastpipe" => Some(state.shell_opts.lastpipe),
                    "autocd" => Some(state.shell_opts.autocd),
                    "cdspell" => Some(state.shell_opts.cdspell),
                    "checkwinsize" => Some(state.shell_opts.checkwinsize),
                    "inherit_errexit" => Some(state.shell_opts.inherit_errexit),
                    _ => None,
                };

                match value {
                    Some(true) => println!("{}\ton", opt),
                    Some(false) => println!("{}\toff", opt),
                    None => {
                        eprintln!("rsh: shopt: {}: invalid option name", opt);
                        exit_code = 1;
                    }
                }
            }
        }
    }

    exit_code
}

fn print_shopt_options(opts: &crate::environment::ShellOpts) {
    // Print all options and their status (like bash)
    let options = vec![
        ("autocd", opts.autocd),
        ("cdspell", opts.cdspell),
        ("checkwinsize", opts.checkwinsize),
        ("dotglob", opts.dotglob),
        ("extglob", opts.extglob),
        ("failglob", opts.failglob),
        ("inherit_errexit", opts.inherit_errexit),
        ("lastpipe", opts.lastpipe),
        ("nocaseglob", opts.nocaseglob),
        ("noglob", opts.noglob),
        ("nullglob", opts.nullglob),
    ];

    for (name, value) in options {
        if value {
            println!("{}\ton", name);
        } else {
            println!("{}\toff", name);
        }
    }
}

// ============================================================
// Structured data pipeline builtins (Phase 9)
// ============================================================

fn builtin_from_json() -> i32 {
    let records = crate::structured::read_json_stdin();
    crate::structured::write_json_stdout(&records);
    0
}

fn builtin_to_json() -> i32 {
    // Read JSON from stdin and re-serialize (identity, but normalizes)
    let records = crate::structured::read_json_stdin();
    crate::structured::write_json_stdout(&records);
    0
}

fn builtin_to_table() -> i32 {
    let records = crate::structured::read_json_stdin();
    let table = crate::structured::to_table(&records);
    print!("{}", table);
    0
}

fn builtin_where(args: &[String]) -> i32 {
    if args.len() < 3 {
        eprintln!("Usage: where <field> <op> <value>");
        return 1;
    }
    let field = &args[0];
    let op = &args[1];
    let value = &args[2];
    let records = crate::structured::read_json_stdin();
    let filtered = crate::structured::filter_where(&records, field, op, value);
    crate::structured::write_json_stdout(&filtered);
    0
}

fn builtin_sort_by(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: sort-by <field> [-r]");
        return 1;
    }
    let field = &args[0];
    let reverse = args.get(1).map(|s| s == "-r").unwrap_or(false);
    let mut records = crate::structured::read_json_stdin();
    crate::structured::sort_by(&mut records, field, reverse);
    crate::structured::write_json_stdout(&records);
    0
}

fn builtin_select(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: select <field1> [field2] ...");
        return 1;
    }
    let fields: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let records = crate::structured::read_json_stdin();
    let projected = crate::structured::select_fields(&records, &fields);
    crate::structured::write_json_stdout(&projected);
    0
}

// ============================================================
// bookmark (Feature 10)
// ============================================================

fn builtin_bookmark(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: bookmark <add|go|ls|rm> [args...]");
        return 1;
    }
    match args[0].as_str() {
        "add" => {
            let name = match args.get(1) {
                Some(n) => n.clone(),
                None => { eprintln!("Usage: bookmark add <name> [path]"); return 1; }
            };
            let path = args.get(2)
                .map(|p| p.clone())
                .unwrap_or_else(|| std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| ".".to_string()));
            if let Ok(mut db) = crate::bookmarks::get_bookmark_db().lock() {
                db.add(&name, &path);
                println!("Bookmark '{}' -> {}", name, path);
            }
            0
        }
        "go" => {
            let name = match args.get(1) {
                Some(n) => n,
                None => { eprintln!("Usage: bookmark go <name>"); return 1; }
            };
            let path = {
                if let Ok(db) = crate::bookmarks::get_bookmark_db().lock() {
                    db.get(name).cloned()
                } else {
                    None
                }
            };
            match path {
                Some(path) => {
                    if let Err(e) = std::env::set_current_dir(&path) {
                        eprintln!("rsh: bookmark go: {}: {}", path, e);
                        return 1;
                    }
                    state.env_vars.insert("OLDPWD".to_string(),
                        state.env_vars.get("PWD").cloned().unwrap_or_default());
                    state.env_vars.insert("PWD".to_string(), path);
                    let chpwd = state.hooks.chpwd.clone();
                    crate::hooks::run_hooks(&chpwd, state);
                    0
                }
                None => {
                    eprintln!("rsh: bookmark '{}' not found", name);
                    1
                }
            }
        }
        "ls" => {
            if let Ok(db) = crate::bookmarks::get_bookmark_db().lock() {
                for (name, path) in db.list() {
                    println!("  {:<16} {}", name, path);
                }
            }
            0
        }
        "rm" => {
            let name = match args.get(1) {
                Some(n) => n,
                None => { eprintln!("Usage: bookmark rm <name>"); return 1; }
            };
            if let Ok(mut db) = crate::bookmarks::get_bookmark_db().lock() {
                if db.remove(name) {
                    println!("Removed bookmark '{}'", name);
                    0
                } else {
                    eprintln!("rsh: bookmark '{}' not found", name);
                    1
                }
            } else {
                1
            }
        }
        _ => {
            eprintln!("Usage: bookmark <add|go|ls|rm> [args...]");
            1
        }
    }
}

// ============================================================
// Enhanced structured data pipeline builtins (Feature 13)
// ============================================================

fn builtin_from_csv() -> i32 {
    let records = crate::structured::read_csv_stdin();
    crate::structured::write_json_stdout(&records);
    0
}

fn builtin_group_by(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: group-by <field>");
        return 1;
    }
    let records = crate::structured::read_json_stdin();
    let grouped = crate::structured::group_by(&records, &args[0]);
    let out = serde_json::to_string_pretty(&grouped).unwrap_or_default();
    println!("{}", out);
    0
}

fn builtin_unique(args: &[String]) -> i32 {
    let field = args.first().map(|s| s.as_str());
    let records = crate::structured::read_json_stdin();
    let unique = crate::structured::unique(&records, field);
    crate::structured::write_json_stdout(&unique);
    0
}

fn builtin_count() -> i32 {
    let records = crate::structured::read_json_stdin();
    println!("{}", crate::structured::count(&records));
    0
}

fn builtin_math(args: &[String]) -> i32 {
    if args.len() < 2 {
        eprintln!("Usage: math <sum|avg|min|max> <field>");
        return 1;
    }
    let op = &args[0];
    let field = &args[1];
    let records = crate::structured::read_json_stdin();
    match crate::structured::math_op(&records, op, field) {
        Some(result) => { println!("{}", result); 0 }
        None => { eprintln!("math: no numeric values for field '{}'", field); 1 }
    }
}
