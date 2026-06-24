/// AST executor: fork/exec, pipes, redirects, compound commands.

use crate::builtins;
use crate::environment::ShellState;
use crate::expand::{expand_word_to_string, expand_words};
use crate::parser::ast::*;
use crate::signal;

use nix::errno::Errno;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{close, execvp, fork, pipe, setpgid, tcsetpgrp, ForkResult, Pid};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::os::unix::io::{IntoRawFd, RawFd, AsRawFd, OwnedFd, BorrowedFd};
use std::io::Write;

fn shell_error(msg: &str) {
    if atty::is(atty::Stream::Stderr) {
        eprintln!("\x1b[1;31mrsh:\x1b[0m {}", msg);
    } else {
        eprintln!("rsh: {}", msg);
    }
}

fn suggest_command(cmd: &str, state: &mut ShellState) -> Option<String> {
    let mut best: Option<(String, usize)> = None;
    let cache = state.path_cache().clone();
    for candidate in cache.iter() {
        let dist = edit_distance(cmd, candidate);
        if dist <= 2 && dist < cmd.len() {
            match &best {
                Some((_, d)) if dist < *d => best = Some((candidate.clone(), dist)),
                None => best = Some((candidate.clone(), dist)),
                _ => {}
            }
        }
    }
    for name in builtins::BUILTIN_NAMES {
        let dist = edit_distance(cmd, name);
        if dist <= 2 && dist < cmd.len() {
            match &best {
                Some((_, d)) if dist < *d => best = Some((name.to_string(), dist)),
                None => best = Some((name.to_string(), dist)),
                _ => {}
            }
        }
    }
    best.map(|(s, _)| s)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m == 0 { return n; }
    if n == 0 { return m; }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i-1] == b[j-1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j-1] + 1).min(prev[j-1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

/// Give terminal foreground to `pgrp`, then wait for the process, then reclaim
/// the terminal for the shell's own process group.
fn wait_for_fg(pid: Pid, state: &mut ShellState) -> i32 {
    let shell_pgid = nix::unistd::getpgrp();
    tcsetpgrp(std::io::stdin(), pid).ok();

    let status = match waitpid(pid, Some(WaitPidFlag::WUNTRACED)) {
        Ok(WaitStatus::Exited(_, code)) => code,
        Ok(WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
        Ok(WaitStatus::Stopped(_, _)) => {
            let cmd_str = format!("(pid {})", pid);
            let jid = state.jobs.add(pid, cmd_str.clone());
            if let Some(job) = state.jobs.get_by_id(jid) {
                job.status = crate::job::JobStatus::Stopped;
                eprintln!("\n[{}]+  Stopped                    {}", jid, cmd_str);
            }
            148
        }
        _ => 1,
    };

    tcsetpgrp(std::io::stdin(), shell_pgid).ok();
    status
}

fn child_exit(code: i32) -> ! {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    unsafe { nix::libc::_exit(code) }
}

fn exec_error_info(_cmd_name: &str) -> (&'static str, i32) {
    match Errno::last() {
        Errno::EACCES => ("Permission denied", 126),
        Errno::ENOEXEC => ("cannot execute binary file", 126),
        Errno::ENOENT => ("command not found", 127),
        _ => ("command not found", 127),
    }
}

pub fn execute_program(commands: &[CompleteCommand], state: &mut ShellState) -> i32 {
    let mut last = 0;
    for cmd in commands {
        last = execute_complete_command(cmd, state);
        if last != 0 {
            fire_err_trap(state);
        }
    }
    last
}

fn fire_err_trap(state: &mut ShellState) {
    if let Some(action) = state.traps.get("ERR").cloned() {
        if !action.is_empty() {
            if let Ok(cmds) = crate::parser::parse(&action) {
                for cmd in &cmds {
                    execute_complete_command(cmd, state);
                }
            }
        }
    }
}

pub fn execute_complete_command(cmd: &CompleteCommand, state: &mut ShellState) -> i32 {
    // Sweep up any finished process-substitution children (non-blocking).
    state.reap_procsubs();
    // Phase 5b: drop inline-closure stash from the prior top-level command.
    state.inline_closures.clear();
    if cmd.background {
        // Fork for background execution
        match unsafe { fork() } {
            Ok(ForkResult::Child) => {
                signal::reset_child_signals();
                let pid = nix::unistd::getpid();
                setpgid(pid, pid).ok();
                let code = execute_and_or(&cmd.list, state);
                child_exit(code);
            }
            Ok(ForkResult::Parent { child }) => {
                setpgid(child, child).ok();
                state.last_bg_pid = Some(child.as_raw() as u32);
                if !cmd.disown {
                    let cmd_str = "(background)";
                    let jid = state.jobs.add(child, cmd_str.to_string());
                    eprintln!("[{}] {}", jid, child);
                }
                state.last_exit_code = 0;
                return 0;
            }
            Err(e) => {
                eprintln!("rsh: fork failed: {}", e);
                return 1;
            }
        }
    }

    let code = execute_and_or(&cmd.list, state);
    state.last_exit_code = code;
    code
}

fn execute_and_or(list: &AndOrList, state: &mut ShellState) -> i32 {
    let mut code = execute_pipeline(&list.first, state);

    for (conn, pipeline) in &list.rest {
        match conn {
            Connector::And => {
                if code == 0 {
                    code = execute_pipeline(pipeline, state);
                }
            }
            Connector::Or => {
                if code != 0 {
                    code = execute_pipeline(pipeline, state);
                }
            }
        }
    }

    code
}

/// True if `cmd` is a `Simple` command whose head literal names a value-aware
/// builtin AND has no redirects / assignments. Conservative: any expansion
/// in the command name disqualifies (we don't want to side-effect expansion).
fn is_value_aware_command(cmd: &Command) -> bool {
    let simple = match cmd {
        Command::Simple(s) => s,
        _ => return false,
    };
    if !simple.redirects.is_empty() || !simple.assignments.is_empty() {
        return false;
    }
    let first = match simple.words.first() {
        Some(w) => w,
        None => return false,
    };
    // Only accept the trivial case: a single literal word part.
    if first.len() != 1 {
        return false;
    }
    let name = match &first[0] {
        WordPart::Literal(s) => s.as_str(),
        _ => return false,
    };
    crate::value_builtins::is_value_aware_in_pipeline(name)
}

/// Run a pipeline composed entirely of value-aware builtins in-process.
fn execute_value_pipeline(cmds: &[Command], state: &mut ShellState) -> i32 {
    use crate::pipeline_data::PipelineData;
    use std::io::Read;

    // If our own stdin is a pipe (not a tty), the user is feeding bytes into
    // the first value-aware stage. Read them eagerly. (Phase 5a is non-streaming.)
    let mut data = if atty::is(atty::Stream::Stdin) {
        PipelineData::Empty
    } else {
        let mut buf = Vec::new();
        let _ = std::io::stdin().lock().read_to_end(&mut buf);
        if buf.is_empty() { PipelineData::Empty } else { PipelineData::Bytes(buf) }
    };
    for (i, cmd) in cmds.iter().enumerate() {
        let simple = match cmd {
            Command::Simple(s) => s,
            _ => unreachable!("is_value_aware_command gate"),
        };
        // Expand the args (the head is known to be a literal, so expand_words is fine).
        let expanded = expand_words(&simple.words, state);
        if expanded.is_empty() {
            continue;
        }
        let name = &expanded[0];
        let args = &expanded[1..];
        let f = match crate::value_builtins::VALUE_BUILTINS.get(name.as_str()) {
            Some(f) => *f,
            None => {
                eprintln!("rsh: {}: not value-aware (shouldn't happen)", name);
                return 1;
            }
        };
        match f(data, args, state) {
            Ok(out) => data = out,
            Err(code) => return code,
        }
        // Last command should print to stdout
        if i == cmds.len() - 1 {
            if let Err(e) = data.write_to_stdout() {
                eprintln!("rsh: {}", e);
                return 1;
            }
            data = PipelineData::Empty;
        }
    }
    0
}

fn execute_pipeline(pipeline: &Pipeline, state: &mut ShellState) -> i32 {
    let cmds = &pipeline.commands;

    if cmds.len() == 1 {
        let code = execute_command(&cmds[0], state);
        state.pipestatus = vec![code];
        state.set_array("PIPESTATUS", vec![code.to_string()]);
        return if pipeline.negated { if code == 0 { 1 } else { 0 } } else { code };
    }

    // Phase 5a: if every command is a value-aware builtin with no redirects /
    // assignments / etc., run the whole pipeline in-process without forking.
    if cmds.iter().all(|c| is_value_aware_command(c)) {
        let code = execute_value_pipeline(cmds, state);
        state.pipestatus = vec![code];
        state.set_array("PIPESTATUS", vec![code.to_string()]);
        return if pipeline.negated { if code == 0 { 1 } else { 0 } } else { code };
    }

    let mut prev_read_fd: Option<RawFd> = None;
    let mut child_pids: Vec<Pid> = Vec::new();
    let mut pgid = Pid::from_raw(0);

    for (i, cmd) in cmds.iter().enumerate() {
        let is_last = i == cmds.len() - 1;

        let (read_fd, write_fd): (Option<RawFd>, Option<RawFd>) = if !is_last {
            match pipe() {
                Ok((r, w)) => (Some(r.into_raw_fd()), Some(w.into_raw_fd())),
                Err(e) => {
                    eprintln!("rsh: pipe failed: {}", e);
                    if let Some(fd) = prev_read_fd { close(fd).ok(); }
                    return 1;
                }
            }
        } else {
            (None, None)
        };

        state.fork_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match unsafe { fork() } {
            Ok(ForkResult::Child) => {
                signal::reset_child_signals();
                state.interactive = false;
                let my_pid = nix::unistd::getpid();
                let target_pgid = if pgid.as_raw() == 0 { my_pid } else { pgid };
                setpgid(my_pid, target_pgid).ok();

                if let Some(fd) = prev_read_fd {
                    dup2_raw(fd, 0).ok();
                    close(fd).ok();
                }
                if let Some(fd) = write_fd {
                    dup2_raw(fd, 1).ok();
                    close(fd).ok();
                }
                if let Some(fd) = read_fd {
                    close(fd).ok();
                }

                let code = execute_command_in_pipeline_child(cmd, state);
                child_exit(code);
            }
            Ok(ForkResult::Parent { child }) => {
                if pgid.as_raw() == 0 {
                    pgid = child;
                }
                setpgid(child, pgid).ok();
                child_pids.push(child);
                if let Some(fd) = write_fd {
                    close(fd).ok();
                }
                if let Some(fd) = prev_read_fd {
                    close(fd).ok();
                }
                prev_read_fd = read_fd;
            }
            Err(e) => {
                eprintln!("rsh: fork failed: {}", e);
                return 1;
            }
        }
    }

    let shell_pgid = nix::unistd::getpgrp();
    if state.interactive && pgid.as_raw() != 0 {
        tcsetpgrp(std::io::stdin(), pgid).ok();
    }

    let mut last_status = 0;
    let mut pipestatus = Vec::new();
    for pid in child_pids {
        match waitpid(pid, None) {
            Ok(WaitStatus::Exited(_, code)) => { pipestatus.push(code); last_status = code; }
            Ok(WaitStatus::Signaled(_, sig, _)) => { let c = 128 + sig as i32; pipestatus.push(c); last_status = c; }
            _ => { pipestatus.push(1); last_status = 1; }
        }
    }

    if state.interactive {
        tcsetpgrp(std::io::stdin(), shell_pgid).ok();
    }
    if state.shell_opts.pipefail {
        if let Some(&code) = pipestatus.iter().find(|&&c| c != 0) {
            last_status = code;
        }
    }
    state.pipestatus = pipestatus.clone();
    state.set_array("PIPESTATUS", pipestatus.iter().map(|c| c.to_string()).collect());

    if pipeline.negated { if last_status == 0 { 1 } else { 0 } } else { last_status }
}

fn execute_command(cmd: &Command, state: &mut ShellState) -> i32 {
    match cmd {
        Command::Simple(simple) => execute_simple(simple, state),
        Command::Compound(compound) => execute_compound(compound, state),
        Command::FunctionDef { name, body } => {
            state.functions.insert(name.clone(), *body.clone());
            0
        }
    }
}

fn execute_command_in_pipeline_child(cmd: &Command, state: &mut ShellState) -> i32 {
    match cmd {
        Command::Simple(simple) => execute_simple_with_mode(simple, state, false),
        _ => execute_command(cmd, state),
    }
}

fn execute_assignment(assign: &Assignment, state: &mut ShellState) {
    if let Some(ref array_words) = assign.array_value {
        // Array assignment: arr=(a b c)
        let values: Vec<String> = array_words.iter()
            .map(|w| expand_word_to_string(w, state))
            .collect();
        if assign.append {
            let arr = state.arrays.entry(assign.name.clone()).or_default();
            arr.extend(values);
        } else {
            state.arrays.insert(assign.name.clone(), values);
        }
    } else if let Some(ref index) = assign.index {
        // Indexed assignment: arr[idx]=value
        let value = expand_word_to_string(&assign.value, state);
        state.set_array_element(&assign.name, index, &value);
    } else if assign.append {
        // String append: var+=value
        let value = expand_word_to_string(&assign.value, state);
        if state.is_array(&assign.name) {
            let arr = state.arrays.entry(assign.name.clone()).or_default();
            arr.push(value);
        } else {
            let old = state.get_var(&assign.name).unwrap_or("").to_string();
            state.set_var(&assign.name, &format!("{}{}", old, value));
        }
    } else {
        let value = expand_word_to_string(&assign.value, state);
        state.set_var(&assign.name, &value);
    }
}

/// `let NAME = ...` form: `=` must be the entire third word (a bare literal).
/// Bash arithmetic `let "x = 1+2"` quotes the assignment into one word.
fn is_typed_let(words: &[Word]) -> bool {
    if words.len() < 4 { return false; }
    let head_is_let = matches!(words[0].as_slice(), [WordPart::Literal(s)] if s == "let");
    let eq_is_bare = matches!(words[2].as_slice(), [WordPart::Literal(s)] if s == "=");
    head_is_let && eq_is_bare && is_simple_ident(&words[1])
}

fn is_simple_ident(w: &Word) -> bool {
    match w.as_slice() {
        [WordPart::Literal(s)] => !s.is_empty()
            && s.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
            && s.chars().all(|c| c.is_alphanumeric() || c == '_'),
        _ => false,
    }
}

fn execute_typed_let(words: &[Word], state: &mut ShellState) -> i32 {
    use crate::value::{Value, ClosureData};
    use std::sync::Arc;

    let name = match &words[1][0] {
        WordPart::Literal(s) => s.clone(),
        _ => return 1,
    };
    let rhs = &words[3..];
    let value = build_let_value(rhs, state);
    state.let_vars.insert(name, value);
    return 0;

    // unused suppressed; type imports above
    #[allow(dead_code)]
    fn _refs(_: Value, _: Arc<ClosureData>) {}
}

fn build_let_value(rhs: &[Word], state: &mut ShellState) -> crate::value::Value {
    use crate::value::{Value, ClosureData};
    use std::sync::Arc;

    // Single closure literal — bind with captured snapshot.
    if rhs.len() == 1 && rhs[0].len() == 1 {
        if let WordPart::Closure { params, body_src } = &rhs[0][0] {
            return Value::Closure(Arc::new(ClosureData {
                params: params.clone(),
                body_src: body_src.clone(),
                captured: state.let_vars.clone(),
            }));
        }
    }
    // Otherwise expand & sniff. Pure-literal RHS (no expansion) goes through
    // JSON; expanded text falls back to JSON if it parses, else String.
    let joined: String = rhs.iter()
        .map(|w| expand_word_to_string(w, state))
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        return Value::Null;
    }
    if let Ok(j) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Value::from_json(j);
    }
    Value::String(trimmed.to_string())
}

/// Apply a closure to a positional argument list, returning the body's value.
/// Establishes a fresh let_vars scope = captured ∪ params, runs the body via
/// the parser/executor, and returns the closure's "result Value".
///
/// Result extraction: closures whose body is a single value-aware pipeline
/// already produce a `PipelineData::Values(...)` we can collect. For the
/// general case (single expression like `$x.age -gt 30`), we re-parse the body
/// and run it through `execute_value_pipeline`-compatible path; if that's not
/// available, we capture stdout as a string Value.
pub fn apply_closure(
    closure: &crate::value::ClosureData,
    args: &[crate::value::Value],
    state: &mut ShellState,
) -> Result<crate::value::Value, i32> {
    use crate::pipeline_data::PipelineData;
    use crate::value::Value;

    // Snapshot let_vars, install captured + bound params for the call.
    let saved = std::mem::take(&mut state.let_vars);
    state.let_vars = closure.captured.clone();
    for (i, p) in closure.params.iter().enumerate() {
        let v = args.get(i).cloned().unwrap_or(Value::Null);
        state.let_vars.insert(p.clone(), v);
    }

    let result = (|| -> Result<Value, i32> {
        let parsed = crate::parser::parse(&closure.body_src).map_err(|_| 2_i32)?;
        let mut last: Value = Value::Null;
        for complete in &parsed {
            // Try the value-aware pipeline path so the closure returns a Value
            // instead of writing to stdout. If the body's first pipeline isn't
            // all-value-aware, fall back to running it for its exit status and
            // returning Bool(exit==0).
            let pipeline = &complete.list.first;
            if !pipeline.commands.is_empty()
                && pipeline.commands.iter().all(is_value_aware_command)
            {
                // The body's pipeline starts with the first argument as input,
                // so `each {|x| select name}` projects from `x`.
                let mut data = match args.first() {
                    Some(v) => PipelineData::Values(vec![v.clone()]),
                    None => PipelineData::Empty,
                };
                for cmd in &pipeline.commands {
                    let simple = match cmd {
                        crate::parser::ast::Command::Simple(s) => s,
                        _ => unreachable!(),
                    };
                    let expanded = expand_words(&simple.words, state);
                    if expanded.is_empty() { continue; }
                    let name = &expanded[0];
                    let extra_args = &expanded[1..];
                    let f = crate::value_builtins::VALUE_BUILTINS.get(name.as_str())
                        .ok_or(2_i32)?;
                    data = f(data, extra_args, state)?;
                }
                last = match data {
                    PipelineData::Values(mut vs) if vs.len() == 1 => vs.remove(0),
                    PipelineData::Values(vs) => Value::List(vs),
                    PipelineData::Bytes(b) => Value::String(String::from_utf8_lossy(&b).to_string()),
                    PipelineData::Empty => Value::Null,
                };
            } else {
                // Fall back: run as a normal command (e.g. `test ...` style).
                let code = execute_complete_command(complete, state);
                last = Value::Bool(code == 0);
            }
        }
        Ok(last)
    })();

    state.let_vars = saved;
    result
}

fn execute_simple(cmd: &SimpleCommand, state: &mut ShellState) -> i32 {
    execute_simple_with_mode(cmd, state, true)
}

fn execute_simple_with_mode(cmd: &SimpleCommand, state: &mut ShellState, fork_external: bool) -> i32 {
    if state.shell_opts.xtrace && !cmd.words.is_empty() {
        let trace: Vec<String> = cmd.words.iter().map(|w| expand_word_to_string(w, state)).collect();
        eprintln!("+ {}", trace.join(" "));
    }

    // Handle assignments only (no command)
    if cmd.words.is_empty() {
        for assign in &cmd.assignments {
            execute_assignment(assign, state);
        }
        return 0;
    }

    // Phase 5b: nushell-style `let NAME = EXPR`.
    // Disambiguated from bash arithmetic `let "x = 1+2"` by requiring `=` as a
    // separate, bare word (no quoting). We access raw WordParts here so a
    // closure literal `{|x|...}` reaches build_value_from_words intact.
    if is_typed_let(&cmd.words) {
        return execute_typed_let(&cmd.words, state);
    }

    // Expand words
    let expanded = expand_words(&cmd.words, state);
    if expanded.is_empty() {
        return 0;
    }

    let cmd_name = &expanded[0];
    let args = &expanded[1..];

    // Check for alias expansion
    if let Some(alias_val) = state.aliases.get(cmd_name).cloned() {
        let full_cmd = if args.is_empty() {
            alias_val
        } else {
            format!("{} {}", alias_val, args.join(" "))
        };
        let parse_result = if let Some(cached) = crate::parser::cache::cache_get(&full_cmd) {
            Some(cached)
        } else if let Ok(parsed) = crate::parser::parse(&full_cmd) {
            crate::parser::cache::cache_insert(full_cmd.clone(), parsed.clone());
            Some(parsed)
        } else {
            None
        };
        if let Some(cmds) = parse_result {
            let removed_alias = state.aliases.remove(cmd_name);
            let mut last = 0;
            for c in &cmds {
                last = execute_complete_command(c, state);
            }
            if let Some(alias) = removed_alias {
                state.aliases.insert(cmd_name.clone(), alias);
            }
            return last;
        }
    }

    // Check for function
    if let Some(func_body) = state.functions.get(cmd_name).cloned() {
        state.push_local_scope();
        state.push_positional_params(args.to_vec());
        let code = execute_compound(&func_body, state);
        state.pop_positional_params();
        state.pop_local_scope();

        // Handle return statement in function
        let return_code = if state.return_requested {
            let ret = state.return_value;
            state.return_requested = false;
            state.return_value = 0;
            ret
        } else {
            code
        };

        return return_code;
    }

    // Check for builtin
    if builtins::is_builtin(cmd_name) {
        let saved_fds = setup_redirects(&cmd.redirects, state);
        let saved_vars: Vec<(String, Option<String>)> = cmd.assignments.iter().map(|a| {
            let old = state.get_var(&a.name).map(|s| s.to_string());
            let val = expand_word_to_string(&a.value, state);
            state.set_var(&a.name, &val);
            (a.name.clone(), old)
        }).collect();

        let code = builtins::run_builtin(cmd_name, &args.to_vec(), state);

        for (name, old) in saved_vars {
            match old {
                Some(v) => state.set_var(&name, &v),
                None => state.unset_var(&name),
            }
        }
        restore_fds(saved_fds);
        return code;
    }

    // External command - fork and exec
    if !fork_external {
        apply_redirects_in_child(&cmd.redirects, state);

        for assign in &cmd.assignments {
            let val = expand_word_to_string(&assign.value, state);
            std::env::set_var(&assign.name, &val);
        }

        let c_cmd = CString::new(cmd_name.as_str()).unwrap_or_default();
        let c_args: Vec<CString> = expanded.iter()
            .map(|s| CString::new(s.as_str()).unwrap_or_default())
            .collect();

        let _ = execvp(&c_cmd, &c_args);
        let (msg, code) = exec_error_info(&cmd_name);
        shell_error(&format!("{}: {}", cmd_name, msg));
        if code == 127 {
            if let Some(suggestion) = suggest_command(&cmd_name, state) {
                eprintln!("\x1b[2;33m       did you mean '{}'?\x1b[0m", suggestion);
            }
        }
        return code;
    }

    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            signal::reset_child_signals();
            let pid = nix::unistd::getpid();
            setpgid(pid, pid).ok();

            apply_redirects_in_child(&cmd.redirects, state);

            for assign in &cmd.assignments {
                let val = expand_word_to_string(&assign.value, state);
                std::env::set_var(&assign.name, &val);
            }

            let c_cmd = CString::new(cmd_name.as_str()).unwrap_or_default();
            let c_args: Vec<CString> = expanded.iter()
                .map(|s| CString::new(s.as_str()).unwrap_or_default())
                .collect();

            let _ = execvp(&c_cmd, &c_args);
            let (msg, code) = exec_error_info(&cmd_name);
            shell_error(&format!("{}: {}", cmd_name, msg));
            child_exit(code);
        }
        Ok(ForkResult::Parent { child }) => {
            setpgid(child, child).ok();
            let exit_code = if state.interactive {
                wait_for_fg(child, state)
            } else {
                match waitpid(child, None) {
                    Ok(WaitStatus::Exited(_, code)) => code,
                    Ok(WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
                    _ => 1,
                }
            };
            if exit_code == 127 {
                if let Some(suggestion) = suggest_command(&cmd_name, state) {
                    eprintln!("\x1b[2;33m       did you mean '{}'?\x1b[0m", suggestion);
                }
            }
            exit_code
        }
        Err(e) => {
            shell_error(&format!("fork failed: {}", e));
            1
        }
    }
}

// Helper function to wrap compound command execution with redirect handling
fn with_redirects<F>(redirects: &[Redirect], state: &mut ShellState, f: F) -> i32
where
    F: FnOnce(&mut ShellState) -> i32,
{
    let saved = setup_redirects(redirects, state);
    let result = f(state);
    restore_fds(saved);
    result
}

pub fn execute_compound(cmd: &CompoundCommand, state: &mut ShellState) -> i32 {
    match cmd {
        CompoundCommand::BraceGroup { body, redirects } => {
            with_redirects(redirects, state, |state| {
                execute_command_list(body, state)
            })
        }
        CompoundCommand::Subshell { body, redirects } => {
            match unsafe { fork() } {
                Ok(ForkResult::Child) => {
                    signal::reset_child_signals();
                    let pid = nix::unistd::getpid();
                    setpgid(pid, pid).ok();
                    apply_redirects_in_child(redirects, state);
                    let code = execute_command_list(body, state);
                    child_exit(code);
                }
                Ok(ForkResult::Parent { child }) => {
                    setpgid(child, child).ok();
                    if state.interactive {
                        wait_for_fg(child, state)
                    } else {
                        match waitpid(child, None) {
                            Ok(WaitStatus::Exited(_, code)) => code,
                            _ => 1,
                        }
                    }
                }
                Err(_) => 1,
            }
        }
        CompoundCommand::If { conditions, else_branch, redirects } => {
            with_redirects(redirects, state, |state| {
                let mut code = 0;
                let mut matched = false;

                for (condition, body) in conditions {
                    let cond_code = execute_condition(condition, state);
                    if cond_code == 0 {
                        code = execute_command_list(body, state);
                        matched = true;
                        break;
                    }
                }

                if !matched {
                    if let Some(else_body) = else_branch {
                        code = execute_command_list(else_body, state);
                    }
                }
                code
            })
        }
        CompoundCommand::For { var, words, body, redirects } => {
            with_redirects(redirects, state, |state| {
                let word_list = match words {
                    Some(ws) => expand_words(ws, state),
                    None => state.positional_params.clone(),
                };

                let mut code = 0;
                for w in &word_list {
                    state.set_var(var, w);
                    code = execute_command_list(body, state);

                    // Check for break/continue control flow
                    if state.loop_break {
                        state.loop_break = false;
                        break;
                    }
                    if state.loop_continue {
                        state.loop_continue = false;
                        continue;
                    }
                }
                code
            })
        }
        CompoundCommand::CStyleFor { init, condition, update, body, redirects } => {
            with_redirects(redirects, state, |state| {
                // Execute init expression
                if !init.is_empty() {
                    let _ = crate::expand::expand_arithmetic(init, state);
                }

                let mut code = 0;
                loop {
                    // Check condition
                    let cond_result = if condition.is_empty() {
                        // Empty condition means infinite loop (like for ((;;)))
                        true
                    } else {
                        let cond_str = crate::expand::expand_arithmetic(condition, state);
                        cond_str.parse::<i32>().unwrap_or(0) != 0
                    };

                    if !cond_result {
                        break;
                    }

                    // Execute body
                    code = execute_command_list(body, state);

                    // Check for break/continue
                    if state.loop_break {
                        state.loop_break = false;
                        break;
                    }
                    if state.loop_continue {
                        state.loop_continue = false;
                        // Continue to update without breaking
                    }

                    // Execute update expression
                    if !update.is_empty() {
                        let _ = crate::expand::expand_arithmetic(update, state);
                    }
                }

                code
            })
        }
        CompoundCommand::While { condition, body, redirects } => {
            with_redirects(redirects, state, |state| {
                let mut code = 0;
                loop {
                    let cond = execute_condition(condition, state);
                    if cond != 0 { break; }
                    code = execute_command_list(body, state);
                    if state.loop_break {
                        state.loop_break = false;
                        break;
                    }
                    if state.loop_continue {
                        state.loop_continue = false;
                    }
                }
                code
            })
        }
        CompoundCommand::Until { condition, body, redirects } => {
            with_redirects(redirects, state, |state| {
                let mut code = 0;
                loop {
                    let cond = execute_condition(condition, state);
                    if cond == 0 { break; }
                    code = execute_command_list(body, state);
                    if state.loop_break {
                        state.loop_break = false;
                        break;
                    }
                    if state.loop_continue {
                        state.loop_continue = false;
                    }
                }
                code
            })
        }
        CompoundCommand::Case { word, arms, redirects } => {
            with_redirects(redirects, state, |state| {
                let value = expand_word_to_string(word, state);
                let mut last = 0;
                let mut i = 0;
                // `fall` is true when the previous arm ended with ;& and we must
                // run this arm's body unconditionally.
                let mut fall = false;
                while i < arms.len() {
                    let arm = &arms[i];
                    let hit = fall || arm.patterns.iter().any(|p| {
                        let pat = expand_word_to_string(p, state);
                        match_pattern(&value, &pat)
                    });
                    if hit {
                        last = execute_command_list(&arm.body, state);
                        match arm.terminator {
                            CaseTerminator::Break => return last,
                            CaseTerminator::FallThrough => { fall = true; i += 1; }
                            CaseTerminator::ContinueMatch => { fall = false; i += 1; }
                        }
                    } else {
                        i += 1;
                    }
                }
                last
            })
        }
        CompoundCommand::Select { var, words, body, redirects } => {
            with_redirects(redirects, state, |state| {
                // Expand items list
                let items = match words {
                    Some(ws) => expand_words(ws, state),
                    None => state.positional_params.clone(),
                };

                if items.is_empty() {
                    return 0;
                }

                let mut code = 0;
                loop {
                    // Display menu
                    for (i, item) in items.iter().enumerate() {
                        println!("{}) {}", i + 1, item);
                    }

                    // Get PS3 prompt (default "#? ")
                    let ps3 = state.get_var("PS3").unwrap_or("#? ").to_string();
                    eprint!("{}", ps3);
                    use std::io::Write;
                    std::io::stderr().flush().ok();

                    // Read user input
                    let mut reply = String::new();
                    match std::io::stdin().read_line(&mut reply) {
                        Ok(0) => {
                            // EOF reached
                            code = 0;
                            break;
                        }
                        Ok(_) => {
                            let reply_trimmed = reply.trim_end_matches('\n').trim_end_matches('\r');
                            state.set_var("REPLY", reply_trimmed);

                            // Validate selection
                            if let Ok(n) = reply_trimmed.parse::<usize>() {
                                if n >= 1 && n <= items.len() {
                                    let selected = &items[n - 1];
                                    state.set_var(var, selected);
                                    code = execute_command_list(body, state);

                                    // Check for break/continue control flow
                                    if state.loop_break {
                                        state.loop_break = false;
                                        break;
                                    }
                                    if state.loop_continue {
                                        state.loop_continue = false;
                                        continue;
                                    }
                                }
                                // Invalid choice (out of range): show menu again without executing body
                            }
                            // Empty input or non-numeric: show menu again
                        }
                        Err(_) => {
                            code = 1;
                            break;
                        }
                    }
                }

                code
            })
        }
        CompoundCommand::Arithmetic { expr, redirects } => {
            with_redirects(redirects, state, |state| {
                // Evaluate the arithmetic expression
                let result_str = crate::expand::expand_arithmetic(expr, state);
                if let Ok(result) = result_str.parse::<i32>() {
                    // In bash, (( expr )) returns 0 if expr != 0, and 1 if expr == 0
                    // This is the opposite of C - expressions are checked for truthiness
                    if result != 0 {
                        0
                    } else {
                        1
                    }
                } else {
                    1
                }
            })
        }
        CompoundCommand::Coproc { name, command, redirects } => {
            with_redirects(redirects, state, |state| {
                // Create two pipes for bidirectional communication
                // Pipe 1: parent writes to child's stdin
                // Pipe 2: parent reads from child's stdout
                let (read_from_parent, write_to_child) = match pipe() {
                    Ok(p) => p,
                    Err(e) => { eprintln!("rsh: pipe failed: {}", e); return 1; }
                };
                let (read_from_child, write_to_parent) = match pipe() {
                    Ok(p) => p,
                    Err(e) => { eprintln!("rsh: pipe failed: {}", e); return 1; }
                };

                match unsafe { fork() } {
                    Ok(ForkResult::Child) => {
                        signal::reset_child_signals();
                        let pid = nix::unistd::getpid();
                        setpgid(pid, pid).ok();

                        // Child: set up pipes
                        let read_fd = read_from_parent.into_raw_fd();
                        let write_fd = write_to_parent.into_raw_fd();

                        // Close parent's ends
                        close(write_to_child.into_raw_fd()).ok();
                        close(read_from_child.into_raw_fd()).ok();

                        // Redirect stdin from parent's write
                        dup2_raw(read_fd, 0).ok();
                        close(read_fd).ok();

                        // Redirect stdout to parent's read
                        dup2_raw(write_fd, 1).ok();
                        close(write_fd).ok();

                        // Execute the command
                        let code = execute_simple(command, state);
                        child_exit(code);
                    }
                    Ok(ForkResult::Parent { child }) => {
                        // Parent: save pipe fds in array variable
                        let coproc_var = name.clone().unwrap_or_else(|| "COPROC".to_string());

                        // Close child's ends
                        close(read_from_parent.into_raw_fd()).ok();
                        close(write_to_parent.into_raw_fd()).ok();

                        // Get raw fds for the pipes (still open)
                        let write_fd = write_to_child.into_raw_fd();
                        let read_fd = read_from_child.into_raw_fd();

                        // Set array: COPROC[0]=read_fd COPROC[1]=write_fd
                        let coproc_array = vec![
                            read_fd.to_string(),
                            write_fd.to_string(),
                        ];
                        state.arrays.insert(coproc_var, coproc_array);

                        // Don't wait for coproc - it runs in background
                        state.last_bg_pid = Some(child.as_raw() as u32);
                        0
                    }
                    Err(_) => {
                        eprintln!("rsh: coproc: fork failed");
                        1
                    }
                }
            })
        }
    }
}

fn execute_command_list(cmds: &[CompleteCommand], state: &mut ShellState) -> i32 {
    let mut code = 0;
    for cmd in cmds {
        code = execute_complete_command(cmd, state);
        if state.loop_break || state.loop_continue || state.return_requested {
            return code;
        }
        if state.shell_opts.errexit && code != 0 {
            return code;
        }
    }
    code
}

fn execute_condition(cmds: &[CompleteCommand], state: &mut ShellState) -> i32 {
    let saved = state.shell_opts.errexit;
    state.shell_opts.errexit = false;
    let code = execute_command_list(cmds, state);
    state.shell_opts.errexit = saved;
    code
}

fn match_pattern(value: &str, pattern: &str) -> bool {
    crate::glob_match::glob_match(pattern, value)
}

// --- Redirect handling ---

struct SavedFd {
    original_fd: RawFd,
    saved_fd: OwnedFd,
}

/// Helper to call dup2 with raw file descriptors
fn dup2_raw(oldfd: RawFd, newfd: RawFd) -> nix::Result<()> {
    unsafe {
        match nix::libc::dup2(oldfd, newfd) {
            -1 => Err(nix::Error::last()),
            _ => Ok(()),
        }
    }
}

/// Materialize here-doc / here-string content into a temp file and return a read
/// fd positioned at the start. Avoids the pipe-buffer deadlock for large content.
fn heredoc_fd(data: &str) -> Option<RawFd> {
    use std::io::{Seek, SeekFrom};
    let mut dir = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    dir.push(format!("rsh-heredoc-{}-{}", pid, nanos));

    let mut file = OpenOptions::new()
        .read(true).write(true).create_new(true)
        .open(&dir).ok()?;
    // Unlink immediately; the open fd keeps the file alive until closed.
    std::fs::remove_file(&dir).ok();
    file.write_all(data.as_bytes()).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    Some(file.into_raw_fd())
}

fn apply_one_redirect(kind: &RedirectKind, fd: RawFd, data: &str, _here_doc_opts: &Option<HereDocOptions>) {
    match kind {
        RedirectKind::Output => {
            if let Ok(file) = File::create(data) {
                let src = file.into_raw_fd();
                dup2_raw(src, fd).ok();
                if src != fd { close(src).ok(); }
            }
        }
        RedirectKind::Append => {
            if let Ok(file) = OpenOptions::new().create(true).append(true).open(data) {
                let src = file.into_raw_fd();
                dup2_raw(src, fd).ok();
                if src != fd { close(src).ok(); }
            }
        }
        RedirectKind::Input => {
            if let Ok(file) = File::open(data) {
                let src = file.into_raw_fd();
                dup2_raw(src, fd).ok();
                if src != fd { close(src).ok(); }
            }
        }
        RedirectKind::HereString | RedirectKind::HereDoc => {
            // Use a temp file rather than a pipe: a blocking write into a pipe with
            // no reader deadlocks once the content exceeds the pipe buffer (~64KB).
            if let Some(src) = heredoc_fd(data) {
                dup2_raw(src, fd).ok();
                close(src).ok();
            }
        }
        RedirectKind::DupOutput | RedirectKind::DupInput => {
            if data == "-" {
                close(fd).ok();
            } else if let Ok(target_fd) = data.parse::<RawFd>() {
                dup2_raw(target_fd, fd).ok();
            }
        }
        RedirectKind::OutputAll => {
            // &> redirects both stdout (1) and stderr (2) to the file
            if let Ok(file) = File::create(data) {
                let src = file.into_raw_fd();
                dup2_raw(src, 1).ok(); // stdout
                dup2_raw(src, 2).ok(); // stderr
                if src != 1 && src != 2 { close(src).ok(); }
            }
        }
        RedirectKind::AppendAll => {
            // &>> appends both stdout (1) and stderr (2) to the file
            if let Ok(file) = OpenOptions::new().create(true).append(true).open(data) {
                let src = file.into_raw_fd();
                dup2_raw(src, 1).ok(); // stdout
                dup2_raw(src, 2).ok(); // stderr
                if src != 1 && src != 2 { close(src).ok(); }
            }
        }
    }
}

fn redirect_fd(redir: &Redirect) -> RawFd {
    match redir.kind {
        RedirectKind::Output | RedirectKind::Append | RedirectKind::DupOutput
        | RedirectKind::OutputAll | RedirectKind::AppendAll => redir.fd.unwrap_or(1),
        RedirectKind::Input | RedirectKind::HereString | RedirectKind::HereDoc
        | RedirectKind::DupInput => redir.fd.unwrap_or(0),
    }
}

fn setup_redirects(redirects: &[Redirect], state: &mut ShellState) -> Vec<SavedFd> {
    let mut saved = Vec::new();
    for redir in redirects {
        let data = if let Some(here_doc_opts) = &redir.here_doc {
            if here_doc_opts.expand_vars {
                expand_word_to_string(&crate::parser::parse_word_parts(&here_doc_opts.content), state)
            } else {
                here_doc_opts.content.clone()
            }
        } else {
            expand_word_to_string(&redir.target, state)
        };

        // For OutputAll and AppendAll, we need to save both stdout and stderr
        match redir.kind {
            RedirectKind::OutputAll | RedirectKind::AppendAll => {
                // Save both fd 1 (stdout) and fd 2 (stderr)
                unsafe {
                    if let Ok(sfd1) = nix::unistd::dup(BorrowedFd::borrow_raw(1)) {
                        saved.push(SavedFd { original_fd: 1, saved_fd: sfd1 });
                    }
                    if let Ok(sfd2) = nix::unistd::dup(BorrowedFd::borrow_raw(2)) {
                        saved.push(SavedFd { original_fd: 2, saved_fd: sfd2 });
                    }
                }
                apply_one_redirect(&redir.kind, 1, &data, &redir.here_doc);
            }
            _ => {
                let fd = redirect_fd(redir);
                // Safe because fd is a valid file descriptor at this point
                unsafe {
                    if let Ok(sfd) = nix::unistd::dup(BorrowedFd::borrow_raw(fd)) {
                        saved.push(SavedFd { original_fd: fd, saved_fd: sfd });
                    }
                }
                apply_one_redirect(&redir.kind, fd, &data, &redir.here_doc);
            }
        }
    }
    saved
}

fn restore_fds(saved: Vec<SavedFd>) {
    for s in saved.into_iter().rev() {
        dup2_raw(s.saved_fd.as_raw_fd(), s.original_fd).ok();
        // OwnedFd will be dropped here and close the fd
    }
}

fn apply_redirects_in_child(redirects: &[Redirect], state: &mut ShellState) {
    for redir in redirects {
        let fd = redirect_fd(redir);

        // Use here-doc content if available, otherwise expand target
        let data = if let Some(here_doc_opts) = &redir.here_doc {
            // Optionally expand variables if requested
            if here_doc_opts.expand_vars {
                expand_word_to_string(&crate::parser::parse_word_parts(&here_doc_opts.content), state)
            } else {
                here_doc_opts.content.clone()
            }
        } else {
            expand_word_to_string(&redir.target, state)
        };

        apply_one_redirect(&redir.kind, fd, &data, &redir.here_doc);
    }
}
