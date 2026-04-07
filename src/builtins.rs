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
    "bookmark", "from-csv", "group-by", "unique", "count", "math", "exec",
    // Stream processing commands
    "sum", "avg", "min", "max", "lines", "stats", "trim", "reverse",
    "upper", "lower",
    // Debug commands
    "debug-trace", "debug-timing", "debug-profile",
    // Data processing commands
    "filter", "map", "head", "tail", "dedupe", "shuffle", "uniq",
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
        "return" => {
            let code = if args.len() > 0 {
                args[0].parse::<i32>().unwrap_or(0)
            } else {
                0
            };
            state.return_requested = true;
            state.return_value = code;
            code
        }
        "break" => {
            state.loop_break = true;
            0
        }
        "continue" => {
            state.loop_continue = true;
            0
        }
        "shift" => builtin_shift(state),
        "exec" => builtin_exec(args, state),
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
        // Stream processing commands
        "sum" => crate::stream::builtin_sum(args),
        "avg" => crate::stream::builtin_avg(args),
        "min" => crate::stream::builtin_min(args),
        "max" => crate::stream::builtin_max(args),
        "lines" => crate::stream::builtin_lines(args),
        "stats" => crate::stream::builtin_stats(args),
        "trim" => crate::stream::builtin_trim(args),
        "reverse" => crate::stream::builtin_reverse(args),
        "upper" => crate::stream::builtin_upper(args),
        "lower" => crate::stream::builtin_lower(args),
        // Debug commands
        "debug-trace" => crate::debug::builtin_debug_trace(args),
        "debug-timing" => crate::debug::builtin_debug_timing(args),
        "debug-profile" => crate::debug::builtin_debug_profile(args),
        // Data processing commands
        "filter" => crate::data::builtin_filter(args),
        "map" => crate::data::builtin_map(args),
        "group-by" => crate::data::builtin_group_by(args, state),
        "select" => crate::data::builtin_select(args),
        "uniq" => crate::data::builtin_uniq(args),
        "head" => crate::data::builtin_head(args),
        "tail" => crate::data::builtin_tail(args),
        "shuffle" => crate::data::builtin_shuffle(args),
        "dedupe" => crate::data::builtin_dedupe(args),
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
    } else if args[0].starts_with('+') || args[0].starts_with('-') {
        // Handle directory stack navigation: cd +N or cd -N
        if let Ok(idx) = args[0][1..].parse::<usize>() {
            if args[0].starts_with('+') {
                if idx < state.dir_stack.len() {
                    state.dir_stack[idx].to_string_lossy().to_string()
                } else {
                    eprintln!("rsh: cd: invalid stack index: +{}", idx);
                    return 1;
                }
            } else {
                // -N means from the end
                if idx <= state.dir_stack.len() {
                    state.dir_stack[state.dir_stack.len() - idx].to_string_lossy().to_string()
                } else {
                    eprintln!("rsh: cd: invalid stack index: -{}", idx);
                    return 1;
                }
            }
        } else {
            args[0].clone()
        }
    } else {
        args[0].clone()
    };

    // Try to change to target directory
    // First try as absolute/relative path
    if let Ok(new_dir) = change_to_directory(&target, state) {
        update_directory_vars(&new_dir, state);
        return 0;
    }

    // Try CDPATH if target doesn't contain /
    if !target.contains('/') {
        if let Some(cdpath_ref) = state.get_var("CDPATH") {
            let cdpath = cdpath_ref.to_string();
            for dir in cdpath.split(':') {
                if dir.is_empty() { continue; }
                let candidate = format!("{}/{}", dir, target);
                if let Ok(new_dir) = change_to_directory(&candidate, state) {
                    println!("{}", new_dir.display());
                    update_directory_vars(&new_dir, state);
                    return 0;
                }
            }
        }
    }

    eprintln!("rsh: cd: {}: No such file or directory", target);
    1
}

fn change_to_directory(path: &str, state: &mut ShellState) -> Result<std::path::PathBuf, std::io::Error> {
    let old = env::current_dir().ok();
    env::set_current_dir(path)?;

    match env::current_dir() {
        Ok(new_dir) => {
            Ok(new_dir)
        }
        Err(e) => {
            if let Some(old_dir) = old {
                let _ = env::set_current_dir(&old_dir);
            }
            Err(e)
        }
    }
}

fn update_directory_vars(new_dir: &std::path::Path, state: &mut ShellState) {
    let old = env::current_dir().ok().map(|p| p.to_string_lossy().to_string());

    let new_str = new_dir.to_string_lossy().to_string();
    state.export_var("PWD", &new_str);
    if let Some(old) = old {
        state.export_var("OLDPWD", &old);
    }

    // z-jump: record directory visit
    if let Ok(mut z_db) = crate::zjump::get_z_db().lock() {
        z_db.add(&new_dir.to_string_lossy());
    }

    // chpwd hooks
    let hooks = state.hooks.chpwd.clone();
    crate::hooks::run_hooks(&hooks, state);
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
                // If in function scope, set to local_vars; otherwise just unset
                if let Some(scope) = state.local_vars_stack.last_mut() {
                    scope.insert(arg.clone(), val);
                }
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
            // Get value from any scope
            if let Some(val) = state.get_var(arg).map(|s| s.to_string()) {
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

/// Use bash to source a script file when rsh's parser can't handle it,
/// then reload environment variables and simple functions back into rsh.
fn source_via_bash(path: &str, state: &mut ShellState) -> i32 {
    // Create a bash script that sources the file and outputs environment variables
    let bash_script = format!(
        r#"
set -a
source "{path}"
set +a

# Output all environment variables in key=value format
declare -p | grep 'declare -x' | sed 's/declare -x //' | sed "s/='/'=/g"

# Output alias definitions for later parsing if needed
alias 2>/dev/null || true

# Output function names
declare -F | awk '{{print $3}}'
"#,
        path = path.replace("'", "\\'")
    );

    // Execute bash script to capture the environment
    match std::process::Command::new("bash")
        .arg("-c")
        .arg(&bash_script)
        .output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            // If bash had errors, print them but continue
            if !stderr.is_empty() && !stderr.contains("warning") {
                eprintln!("rsh: bash source warnings: {}", stderr.trim());
            }

            // Parse exported variables from bash output
            for line in stdout.lines() {
                // Skip function names (no = sign) and aliases
                if line.contains('=') && !line.starts_with("alias") {
                    if let Some(eq_pos) = line.find('=') {
                        let key = &line[..eq_pos];
                        let value = &line[eq_pos + 1..];
                        // Remove quotes if present
                        let value = if (value.starts_with('\'') && value.ends_with('\'')) ||
                                       (value.starts_with('"') && value.ends_with('"')) {
                            &value[1..value.len()-1]
                        } else {
                            value
                        };
                        state.export_var(key, value);
                    }
                }
            }

            // Return success (bash exit code is usually 0 for sourcing)
            if output.status.success() { 0 } else { 1 }
        }
        Err(e) => {
            eprintln!("rsh: source: failed to execute bash fallback: {}", e);
            1
        }
    }
}

fn builtin_source(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() {
        eprintln!("rsh: source: filename argument required");
        return 1;
    }

    let filename = &args[0];
    let additional_args = &args[1..];

    // Try to find the file
    let resolved_path = if Path::new(filename).is_file() {
        // File exists at given path
        filename.to_string()
    } else if !filename.contains('/') {
        // No slashes in path, try multiple sources
        // 1. Try current directory
        if Path::new(filename).is_file() {
            filename.to_string()
        } else if let Some(found) = find_in_path(filename) {
            // 2. Try $PATH
            found
        } else {
            eprintln!("rsh: source: {}: No such file or directory", filename);
            return 1;
        }
    } else {
        // Absolute or relative path doesn't exist
        eprintln!("rsh: source: {}: No such file or directory", filename);
        return 1;
    };

    // Set $0 to script name for this invocation
    let old_0 = state.get_var("0").map(|s| s.to_string());
    state.export_var("0", filename);

    // Manage positional parameters for arguments
    let old_params = state.positional_params.clone();
    let mut new_params = vec![filename.clone()];
    new_params.extend(additional_args.iter().cloned());
    state.positional_params = new_params;

    let result = match std::fs::read_to_string(&resolved_path) {
        Ok(content) => {
            match parser::parse(&content) {
                Ok(commands) => {
                    // Parse succeeded, execute all commands in current shell context
                    let mut last = 0;
                    for cmd in &commands {
                        last = crate::executor::execute_complete_command(cmd, state);
                        // Stop on early return
                        if state.return_requested {
                            state.return_requested = false;
                            break;
                        }
                    }
                    last
                }
                Err(e) => {
                    eprintln!("rsh: source: {}: parse error: {}", resolved_path, e);
                    // Try bash as fallback only for complex scripts
                    source_via_bash(&resolved_path, state)
                }
            }
        }
        Err(e) => {
            eprintln!("rsh: source: {}: {}", resolved_path, e);
            1
        }
    };

    // Restore state
    state.positional_params = old_params;
    if let Some(val) = old_0 {
        state.export_var("0", &val);
    }

    result
}

fn builtin_eval(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() {
        return 0;
    }

    // Join all arguments with space, just like bash does
    let input = args.join(" ");

    // Parse and execute the input
    match parser::parse(&input) {
        Ok(commands) => {
            let mut last = 0;
            for cmd in &commands {
                last = crate::executor::execute_complete_command(cmd, state);
                // Early return doesn't stop eval loop (unlike source)
                if state.loop_break || state.loop_continue {
                    break;
                }
            }
            last
        }
        Err(e) => {
            eprintln!("rsh: eval: parse error: {}", e);
            2
        }
    }
}

fn builtin_read(args: &[String], state: &mut ShellState) -> i32 {
    let mut prompt_str = None;
    let mut silent = false;
    let mut raw = false;
    let mut timeout_secs: Option<f64> = None;
    let mut count_chars: Option<usize> = None;
    let mut delim = '\n';
    let mut exact_count: Option<usize> = None;
    let mut read_array = false;
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
            "-t" => {
                i += 1;
                if i < args.len() {
                    timeout_secs = args[i].parse::<f64>().ok();
                }
            }
            "-n" => {
                i += 1;
                if i < args.len() {
                    count_chars = args[i].parse::<usize>().ok();
                }
            }
            "-N" => {
                i += 1;
                if i < args.len() {
                    exact_count = args[i].parse::<usize>().ok();
                }
            }
            "-d" => {
                i += 1;
                if i < args.len() {
                    let d = &args[i];
                    if !d.is_empty() {
                        delim = d.chars().next().unwrap();
                    }
                }
            }
            "-a" => {
                read_array = true;
            }
            s if s.starts_with('-') => {}
            _ => { var_names.push(&args[i]); }
        }
        i += 1;
    }

    if var_names.is_empty() && !read_array {
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

    let result = if let Some(count) = exact_count {
        // Read exactly N characters
        read_exact_chars(count, &var_names, read_array, state)
    } else if let Some(count) = count_chars {
        // Read up to N characters
        read_limited_chars(count, delim, &var_names, read_array, state)
    } else {
        // Read line with delimiter
        read_line_with_delimiter(delim, raw, &var_names, read_array, state)
    };

    if silent {
        std::process::Command::new("stty").arg("echo").status().ok();
        eprintln!();
    }

    result
}

fn read_exact_chars(count: usize, var_names: &[&str], read_array: bool, state: &mut ShellState) -> i32 {
    use std::io::Read;

    let mut buffer = vec![0u8; count];
    match std::io::stdin().read_exact(&mut buffer) {
        Ok(()) => {
            let line = String::from_utf8_lossy(&buffer).into_owned();
            if read_array {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(arr_name) = var_names.first() {
                    state.arrays.insert(arr_name.to_string(), parts.into_iter().map(|s| s.to_string()).collect());
                }
            } else if var_names.len() == 1 {
                state.set_var(var_names[0], &line);
            }
            0
        }
        Err(_) => 1,
    }
}

fn read_limited_chars(max_count: usize, delim: char, var_names: &[&str], read_array: bool, state: &mut ShellState) -> i32 {
    use std::io::Read;

    let mut buffer = vec![0u8; max_count];
    match std::io::stdin().read(&mut buffer) {
        Ok(n) if n > 0 => {
            buffer.truncate(n);
            let line = String::from_utf8_lossy(&buffer).into_owned();
            if read_array {
                let parts: Vec<&str> = line.split(delim).collect();
                if let Some(arr_name) = var_names.first() {
                    state.arrays.insert(arr_name.to_string(), parts.into_iter().map(|s| s.to_string()).collect());
                }
            } else if var_names.len() == 1 {
                state.set_var(var_names[0], &line);
            } else {
                let parts: Vec<&str> = line.split(delim).collect();
                for (vi, var) in var_names.iter().enumerate() {
                    state.set_var(var, parts.get(vi).unwrap_or(&""));
                }
            }
            0
        }
        _ => 1,
    }
}

fn read_line_with_delimiter(delim: char, raw: bool, var_names: &[&str], read_array: bool, state: &mut ShellState) -> i32 {
    use std::io::BufRead;

    let stdin = std::io::stdin();
    let mut line = String::new();

    let result = match stdin.read_line(&mut line) {
        Ok(0) => 1,
        Ok(_) => {
            let line = line.trim_end_matches('\n').trim_end_matches('\r');
            let line = if !raw {
                line.replace("\\\n", "")
            } else {
                line.to_string()
            };

            if read_array {
                // Get IFS for splitting
                let ifs = state.get_var("IFS").unwrap_or(" \t\n");
                let parts: Vec<&str> = line.split(|c: char| ifs.contains(c)).filter(|s| !s.is_empty()).collect();
                if let Some(arr_name) = var_names.first() {
                    state.arrays.insert(arr_name.to_string(), parts.into_iter().map(|s| s.to_string()).collect());
                }
            } else if var_names.len() == 1 {
                state.set_var(var_names[0], &line);
            } else {
                // Get IFS for splitting
                let ifs = state.get_var("IFS").unwrap_or(" \t\n");
                let parts: Vec<&str> = line.split(|c: char| ifs.contains(c)).filter(|s| !s.is_empty()).collect();
                for (vi, var) in var_names.iter().enumerate() {
                    state.set_var(var, parts.get(vi).unwrap_or(&""));
                }
            }
            0
        }
        Err(_) => 1,
    };

    result
}

fn builtin_test(args: &[String]) -> i32 {
    let args: Vec<&str> = args.iter()
        .map(|s| s.as_str())
        .filter(|s| *s != "]")
        .collect();

    if args.is_empty() { return 1; }

    match parse_test_expr(&args, 0).0 {
        TestResult::True => 0,
        TestResult::False => 1,
        TestResult::Error => 2,
    }
}

#[derive(Debug, Clone, Copy)]
enum TestResult {
    True,
    False,
    Error,
}

fn parse_test_expr(args: &[&str], mut idx: usize) -> (TestResult, usize) {
    let (mut result, new_idx) = parse_or_expr(args, idx);
    (result, new_idx)
}

fn parse_or_expr(args: &[&str], mut idx: usize) -> (TestResult, usize) {
    let (mut left, mut new_idx) = parse_and_expr(args, idx);

    while new_idx < args.len() && args[new_idx] == "-o" {
        new_idx += 1;
        let (right, next_idx) = parse_and_expr(args, new_idx);

        left = match (left, right) {
            (TestResult::True, _) => TestResult::True,
            (_, TestResult::True) => TestResult::True,
            (TestResult::False, TestResult::False) => TestResult::False,
            _ => TestResult::Error,
        };
        new_idx = next_idx;
    }

    (left, new_idx)
}

fn parse_and_expr(args: &[&str], mut idx: usize) -> (TestResult, usize) {
    let (mut left, mut new_idx) = parse_primary(args, idx);

    while new_idx < args.len() && args[new_idx] == "-a" {
        new_idx += 1;
        let (right, next_idx) = parse_primary(args, new_idx);

        left = match (left, right) {
            (TestResult::False, _) => TestResult::False,
            (_, TestResult::False) => TestResult::False,
            (TestResult::True, TestResult::True) => TestResult::True,
            _ => TestResult::Error,
        };
        new_idx = next_idx;
    }

    (left, new_idx)
}

fn parse_primary(args: &[&str], idx: usize) -> (TestResult, usize) {
    if idx >= args.len() {
        return (TestResult::Error, idx);
    }

    // Handle negation
    if args[idx] == "!" {
        let (result, new_idx) = parse_primary(args, idx + 1);
        let negated = match result {
            TestResult::True => TestResult::False,
            TestResult::False => TestResult::True,
            TestResult::Error => TestResult::Error,
        };
        return (negated, new_idx);
    }

    // Handle parentheses
    if args[idx] == "(" {
        let (result, new_idx) = parse_or_expr(args, idx + 1);
        if new_idx < args.len() && args[new_idx] == ")" {
            return (result, new_idx + 1);
        }
        return (TestResult::Error, new_idx);
    }

    // Handle unary operators
    if idx + 1 < args.len() {
        match args[idx] {
            "-n" => return (if !args[idx + 1].is_empty() { TestResult::True } else { TestResult::False }, idx + 2),
            "-z" => return (if args[idx + 1].is_empty() { TestResult::True } else { TestResult::False }, idx + 2),
            "-f" => return (if Path::new(args[idx + 1]).is_file() { TestResult::True } else { TestResult::False }, idx + 2),
            "-d" => return (if Path::new(args[idx + 1]).is_dir() { TestResult::True } else { TestResult::False }, idx + 2),
            "-e" => return (if Path::new(args[idx + 1]).exists() { TestResult::True } else { TestResult::False }, idx + 2),
            "-L" => return (if is_symlink(args[idx + 1]) { TestResult::True } else { TestResult::False }, idx + 2),
            "-p" => return (if is_fifo(args[idx + 1]) { TestResult::True } else { TestResult::False }, idx + 2),
            "-S" => return (if is_socket(args[idx + 1]) { TestResult::True } else { TestResult::False }, idx + 2),
            "-b" => return (if is_block_device(args[idx + 1]) { TestResult::True } else { TestResult::False }, idx + 2),
            "-c" => return (if is_char_device(args[idx + 1]) { TestResult::True } else { TestResult::False }, idx + 2),
            "-s" => {
                let result = if let Ok(m) = std::fs::metadata(args[idx + 1]) {
                    if m.len() > 0 { TestResult::True } else { TestResult::False }
                } else {
                    TestResult::False
                };
                return (result, idx + 2);
            }
            "-r" => return (if is_readable(args[idx + 1]) { TestResult::True } else { TestResult::False }, idx + 2),
            "-w" => return (if is_writable(args[idx + 1]) { TestResult::True } else { TestResult::False }, idx + 2),
            "-x" => return (if is_executable(args[idx + 1]) { TestResult::True } else { TestResult::False }, idx + 2),
            _ => {}
        }
    }

    // Handle binary operators
    if idx + 2 < args.len() {
        match args[idx + 1] {
            "=" | "==" => return (if args[idx] == args[idx + 2] { TestResult::True } else { TestResult::False }, idx + 3),
            "!=" => return (if args[idx] != args[idx + 2] { TestResult::True } else { TestResult::False }, idx + 3),
            "<" => return (if args[idx] < args[idx + 2] { TestResult::True } else { TestResult::False }, idx + 3),
            ">" => return (if args[idx] > args[idx + 2] { TestResult::True } else { TestResult::False }, idx + 3),
            "-eq" => {
                let result = match (args[idx].parse::<i64>(), args[idx + 2].parse::<i64>()) {
                    (Ok(a), Ok(b)) => if a == b { TestResult::True } else { TestResult::False },
                    _ => TestResult::Error,
                };
                return (result, idx + 3);
            }
            "-ne" => {
                let result = match (args[idx].parse::<i64>(), args[idx + 2].parse::<i64>()) {
                    (Ok(a), Ok(b)) => if a != b { TestResult::True } else { TestResult::False },
                    _ => TestResult::Error,
                };
                return (result, idx + 3);
            }
            "-lt" => {
                let result = match (args[idx].parse::<i64>(), args[idx + 2].parse::<i64>()) {
                    (Ok(a), Ok(b)) => if a < b { TestResult::True } else { TestResult::False },
                    _ => TestResult::Error,
                };
                return (result, idx + 3);
            }
            "-le" => {
                let result = match (args[idx].parse::<i64>(), args[idx + 2].parse::<i64>()) {
                    (Ok(a), Ok(b)) => if a <= b { TestResult::True } else { TestResult::False },
                    _ => TestResult::Error,
                };
                return (result, idx + 3);
            }
            "-gt" => {
                let result = match (args[idx].parse::<i64>(), args[idx + 2].parse::<i64>()) {
                    (Ok(a), Ok(b)) => if a > b { TestResult::True } else { TestResult::False },
                    _ => TestResult::Error,
                };
                return (result, idx + 3);
            }
            "-ge" => {
                let result = match (args[idx].parse::<i64>(), args[idx + 2].parse::<i64>()) {
                    (Ok(a), Ok(b)) => if a >= b { TestResult::True } else { TestResult::False },
                    _ => TestResult::Error,
                };
                return (result, idx + 3);
            }
            _ => {}
        }
    }

    // Single argument - check if non-empty string
    if idx + 1 == args.len() {
        return (if !args[idx].is_empty() { TestResult::True } else { TestResult::False }, idx + 1);
    }

    (TestResult::Error, idx)
}

fn is_symlink(path: &str) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn is_fifo(path: &str) -> bool {
    use nix::sys::stat;
    if let Ok(stat) = stat::stat(path) {
        stat.st_mode & 0o170000 == 0o10000
    } else {
        false
    }
}

fn is_socket(path: &str) -> bool {
    use nix::sys::stat;
    if let Ok(stat) = stat::stat(path) {
        stat.st_mode & 0o170000 == 0o140000
    } else {
        false
    }
}

fn is_block_device(path: &str) -> bool {
    use nix::sys::stat;
    if let Ok(stat) = stat::stat(path) {
        stat.st_mode & 0o170000 == 0o60000
    } else {
        false
    }
}

fn is_char_device(path: &str) -> bool {
    use nix::sys::stat;
    if let Ok(stat) = stat::stat(path) {
        stat.st_mode & 0o170000 == 0o20000
    } else {
        false
    }
}

fn is_readable(path: &str) -> bool {
    use nix::unistd;
    unistd::access(std::path::Path::new(path), unistd::AccessFlags::R_OK).is_ok()
}

fn is_writable(path: &str) -> bool {
    use nix::unistd;
    unistd::access(std::path::Path::new(path), unistd::AccessFlags::W_OK).is_ok()
}

fn is_executable(path: &str) -> bool {
    use nix::unistd;
    unistd::access(std::path::Path::new(path), unistd::AccessFlags::X_OK).is_ok()
}

fn cmp_int(a: &str, b: &str, f: fn(i64, i64) -> bool) -> i32 {
    match (a.parse::<i64>(), b.parse::<i64>()) {
        (Ok(a), Ok(b)) => if f(a, b) { 0 } else { 1 },
        _ => 2,
    }
}

fn builtin_set(args: &[String], state: &mut ShellState) -> i32 {
    if args.is_empty() {
        let mut all: Vec<_> = state.env_vars.iter().collect();
        // Also include variables from all local scopes
        for scope in &state.local_vars_stack {
            all.extend(scope.iter());
        }
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
            if let Some(scope) = state.local_vars_stack.last_mut() {
                scope.insert(name.to_string(), value.to_string());
            }
        } else {
            if let Some(scope) = state.local_vars_stack.last_mut() {
                scope.insert(arg.clone(), String::new());
            }
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

fn builtin_exec(args: &[String], _state: &mut ShellState) -> i32 {
    use std::fs::{File, OpenOptions};
    use std::os::unix::io::{IntoRawFd, RawFd};
    use nix::unistd::close;

    fn dup2_raw(oldfd: RawFd, newfd: RawFd) -> Result<(), String> {
        unsafe {
            match nix::libc::dup2(oldfd, newfd) {
                -1 => Err(format!("dup2 failed")),
                _ => Ok(()),
            }
        }
    }

    if args.is_empty() {
        return 0;
    }

    // Simple implementation of exec FD redirection
    // Format: exec FD<file, exec FD>file, exec FD>&FD2, etc.

    for arg in args {
        // Parse FD redirection: "3<file", "1>file", "2>&1", "{fd}>&-", etc.
        let (fd_str, redirect_type, target) = if let Some(pos) = arg.find('<') {
            let fd = &arg[..pos];
            let target = &arg[pos + 1..];
            (fd, "<", target)
        } else if let Some(pos) = arg.find('>') {
            let fd = &arg[..pos];
            if pos + 1 < arg.len() && arg.chars().nth(pos + 1) == Some('>') {
                // >> redirect
                let target = &arg[pos + 2..];
                (fd, ">>", target)
            } else if pos + 1 < arg.len() && arg.chars().nth(pos + 1) == Some('&') {
                // >& redirect
                let target = &arg[pos + 2..];
                (fd, ">&", target)
            } else {
                // > redirect
                let target = &arg[pos + 1..];
                (fd, ">", target)
            }
        } else {
            continue;
        };

        // Parse the FD number (handle {fd} format)
        let fd_clean = fd_str.trim_matches(|c| c == '{' || c == '}');
        let fd: i32 = match fd_clean.parse() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("rsh: exec: invalid file descriptor: {}", fd_str);
                return 1;
            }
        };

        // Execute the redirection
        match redirect_type {
            "<" => {
                // Input redirection: open for reading
                match File::open(target) {
                    Ok(file) => {
                        let src_fd = file.into_raw_fd();
                        if let Err(_) = dup2_raw(src_fd, fd) {
                            eprintln!("rsh: exec: dup2 failed");
                            return 1;
                        }
                        if src_fd != fd {
                            close(src_fd).ok();
                        }
                    }
                    Err(_) => {
                        eprintln!("rsh: exec: cannot open {} for reading", target);
                        return 1;
                    }
                }
            }
            ">" => {
                // Output redirection: open for writing
                match File::create(target) {
                    Ok(file) => {
                        let src_fd = file.into_raw_fd();
                        if let Err(_) = dup2_raw(src_fd, fd) {
                            eprintln!("rsh: exec: dup2 failed");
                            return 1;
                        }
                        if src_fd != fd {
                            close(src_fd).ok();
                        }
                    }
                    Err(_) => {
                        eprintln!("rsh: exec: cannot open {} for writing", target);
                        return 1;
                    }
                }
            }
            ">>" => {
                // Append redirection
                match OpenOptions::new().create(true).append(true).open(target) {
                    Ok(file) => {
                        let src_fd = file.into_raw_fd();
                        if let Err(_) = dup2_raw(src_fd, fd) {
                            eprintln!("rsh: exec: dup2 failed");
                            return 1;
                        }
                        if src_fd != fd {
                            close(src_fd).ok();
                        }
                    }
                    Err(_) => {
                        eprintln!("rsh: exec: cannot open {} for appending", target);
                        return 1;
                    }
                }
            }
            ">&" => {
                if target == "-" {
                    // Close FD
                    close(fd).ok();
                } else {
                    // Duplicate FD
                    match target.parse::<i32>() {
                        Ok(target_fd) => {
                            if let Err(_) = dup2_raw(target_fd, fd) {
                                eprintln!("rsh: exec: dup2 failed");
                                return 1;
                            }
                        }
                        Err(_) => {
                            eprintln!("rsh: exec: invalid target FD: {}", target);
                            return 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    0
}

fn builtin_pushd(args: &[String], state: &mut ShellState) -> i32 {
    let mut print_stack = true;
    let mut args_start = 0;

    // Parse options
    for (i, arg) in args.iter().enumerate() {
        if arg == "-n" {
            print_stack = false;
            args_start = i + 1;
        } else {
            break;
        }
    }

    let remaining_args = &args[args_start..];

    let cwd = env::current_dir().ok();
    let target = if remaining_args.is_empty() {
        // pushd with no args swaps top two directories
        if state.dir_stack.is_empty() {
            eprintln!("rsh: pushd: no other directory");
            return 1;
        }
        match state.dir_stack.pop() {
            Some(d) => d.to_string_lossy().to_string(),
            None => {
                eprintln!("rsh: pushd: no other directory");
                return 1;
            }
        }
    } else if remaining_args[0].starts_with('+') || remaining_args[0].starts_with('-') {
        // Handle stack navigation: pushd +N or pushd -N
        if let Ok(idx) = remaining_args[0][1..].parse::<usize>() {
            if remaining_args[0].starts_with('+') {
                if idx < state.dir_stack.len() {
                    state.dir_stack[idx].to_string_lossy().to_string()
                } else {
                    eprintln!("rsh: pushd: invalid stack index: +{}", idx);
                    return 1;
                }
            } else {
                if idx <= state.dir_stack.len() {
                    state.dir_stack[state.dir_stack.len() - idx].to_string_lossy().to_string()
                } else {
                    eprintln!("rsh: pushd: invalid stack index: -{}", idx);
                    return 1;
                }
            }
        } else {
            remaining_args[0].clone()
        }
    } else {
        remaining_args[0].clone()
    };

    if let Some(cwd) = cwd {
        state.dir_stack.push(cwd);
    }

    match env::set_current_dir(&target) {
        Ok(()) => {
            if let Ok(new_dir) = env::current_dir() {
                state.export_var("PWD", &new_dir.to_string_lossy().to_string());
            }
            if print_stack {
                builtin_dirs(state);
            }
            0
        }
        Err(e) => {
            eprintln!("rsh: pushd: {}: {}", target, e);
            1
        }
    }
}

fn builtin_popd(state: &mut ShellState) -> i32 {
    if state.dir_stack.is_empty() {
        eprintln!("rsh: popd: directory stack empty");
        return 1;
    }

    match state.dir_stack.pop() {
        Some(dir) => {
            match env::set_current_dir(&dir) {
                Ok(()) => {
                    state.export_var("PWD", &dir.to_string_lossy().to_string());
                    builtin_dirs(state);
                    0
                }
                Err(e) => {
                    eprintln!("rsh: popd: {}", e);
                    1
                }
            }
        }
        None => {
            eprintln!("rsh: popd: directory stack empty");
            1
        }
    }
}

fn builtin_dirs(state: &ShellState) -> i32 {
    let cwd = env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
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
        println!("EXIT HUP INT QUIT ABRT ALRM TERM USR1 USR2");
        return 0;
    }

    if args.len() == 1 && args[0] == "-p" {
        for (sig, cmd) in &state.traps {
            println!("trap -- '{}' {}", cmd, sig);
        }
        return 0;
    }

    if args.len() >= 2 {
        let action = &args[0];
        for sig in &args[1..] {
            // Validate signal name
            let sig_lower = sig.to_uppercase();
            let valid_signals = vec![
                "EXIT", "HUP", "INT", "QUIT", "ABRT", "ALRM", "TERM",
                "USR1", "USR2", "PIPE", "CHLD", "TSTP", "TTIN", "TTOU",
                "CONT", "STOP", "KILL", "ILL", "FPE", "SEGV", "BUS",
                "SYS", "TRAP", "CLD", "PWR", "POLL", "PROF", "VTALRM",
                "XCPU", "XFSZ", "IOT", "EMT", "STKFLT", "IO", "ERR",
                "RETURN", "DEBUG"
            ];

            let is_valid = valid_signals.iter().any(|&s| s == sig_lower) || sig_lower.parse::<i32>().is_ok();

            if !is_valid {
                eprintln!("rsh: trap: {} is not a valid signal name", sig);
                return 1;
            }

            if action == "-" || action.is_empty() {
                state.traps.remove(&sig_lower);
            } else {
                state.traps.insert(sig_lower, action.clone());
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
        // Handle name=value or name=()
        let (var_name, value) = if let Some(eq) = name.find('=') {
            (&name[..eq], Some(&name[eq + 1..]))
        } else {
            (*name, None)
        };

        if associative {
            if !state.assoc_arrays.contains_key(var_name) {
                state.assoc_arrays.insert(var_name.to_string(), std::collections::HashMap::new());
            }

            // Parse initialization value like: ([key1]=val1 [key2]=val2)
            if let Some(val) = value {
                if val.starts_with('(') && val.ends_with(')') {
                    let inner = &val[1..val.len()-1].trim();
                    parse_assoc_array_init(var_name, inner, state);
                } else if !val.is_empty() && !val.starts_with('(') {
                    // Handle single value assignment (rare for assoc arrays)
                    state.assoc_arrays.get_mut(var_name).unwrap().insert("0".to_string(), val.to_string());
                }
            }
        } else if indexed {
            if !state.arrays.contains_key(var_name) {
                state.arrays.insert(var_name.to_string(), Vec::new());
            }

            // Parse initialization value like: (val1 val2 val3)
            if let Some(val) = value {
                if val.starts_with('(') && val.ends_with(')') {
                    let inner = &val[1..val.len()-1];
                    let elements: Vec<&str> = inner.split_whitespace().collect();
                    *state.arrays.get_mut(var_name).unwrap() = elements.iter().map(|s| s.to_string()).collect();
                } else if !val.is_empty() && !val.starts_with('(') {
                    // Single value
                    state.arrays.get_mut(var_name).unwrap().push(val.to_string());
                }
            }
        } else {
            // Regular variable
            if let Some(val) = value {
                state.set_var(var_name, val);
            }
        }
    }
    0
}

fn parse_assoc_array_init(var_name: &str, input: &str, state: &mut ShellState) {
    // Parse input like: [key1]=val1 [key2]=val2
    let mut current = input;
    while !current.is_empty() {
        current = current.trim_start();
        if !current.starts_with('[') {
            break;
        }

        // Find closing bracket
        if let Some(bracket_end) = current.find(']') {
            let key = &current[1..bracket_end];
            let rest = &current[bracket_end + 1..];

            // Skip = sign
            if rest.starts_with('=') {
                let value_part = &rest[1..].trim_start();

                // Extract value (quoted or unquoted)
                let (value, next_pos) = if value_part.starts_with('"') {
                    // Quoted value
                    let mut escaped = false;
                    let mut end_pos = 1;
                    for (i, ch) in value_part[1..].char_indices() {
                        if escaped {
                            escaped = false;
                        } else if ch == '\\' {
                            escaped = true;
                        } else if ch == '"' {
                            end_pos = i + 2;
                            break;
                        }
                    }
                    let val = &value_part[1..end_pos-1];
                    (val.to_string(), end_pos)
                } else if value_part.starts_with('\'') {
                    // Single-quoted value
                    if let Some(end) = value_part[1..].find('\'') {
                        let val = &value_part[1..end+1];
                        (val.to_string(), end + 2)
                    } else {
                        break;
                    }
                } else {
                    // Unquoted value (until space or next bracket)
                    let end_pos = value_part.find(|c: char| c == ' ' || c == '[')
                        .unwrap_or(value_part.len());
                    (value_part[..end_pos].to_string(), end_pos)
                };

                // Store in associative array
                state.assoc_arrays.get_mut(var_name)
                    .unwrap()
                    .insert(key.to_string(), value);

                current = &value_part[next_pos..];
            } else {
                break;
            }
        } else {
            break;
        }
    }
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
