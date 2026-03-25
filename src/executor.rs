/// AST executor: fork/exec, pipes, redirects, compound commands.

use crate::builtins;
use crate::environment::ShellState;
use crate::expand::{expand_word_to_string, expand_words};
use crate::parser::ast::*;
use crate::signal;

use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{close, dup2, execvp, fork, pipe, setpgid, tcsetpgrp, ForkResult, Pid};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::os::unix::io::{IntoRawFd, RawFd};

/// Give terminal foreground to `pgrp`, then wait for the process, then reclaim
/// the terminal for the shell's own process group.
fn wait_for_fg(pid: Pid, state: &mut ShellState) -> i32 {
    let shell_pgid = nix::unistd::getpgrp();
    // Give the child's process group the terminal foreground
    tcsetpgrp(std::io::stdin(), pid).ok();

    let status = match waitpid(pid, Some(WaitPidFlag::WUNTRACED)) {
        Ok(WaitStatus::Exited(_, code)) => code,
        Ok(WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
        Ok(WaitStatus::Stopped(_, _)) => {
            // Job got stopped (Ctrl-Z) — add to job table
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

    // Reclaim terminal foreground for the shell
    tcsetpgrp(std::io::stdin(), shell_pgid).ok();
    status
}

pub fn execute_program(commands: &[CompleteCommand], state: &mut ShellState) -> i32 {
    let mut last = 0;
    for cmd in commands {
        last = execute_complete_command(cmd, state);
    }
    last
}

pub fn execute_complete_command(cmd: &CompleteCommand, state: &mut ShellState) -> i32 {
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

fn execute_pipeline(pipeline: &Pipeline, state: &mut ShellState) -> i32 {
    let cmds = &pipeline.commands;

    if cmds.len() == 1 {
        // Single command - no pipe needed
        let code = execute_command(&cmds[0], state);
        return if pipeline.negated { if code == 0 { 1 } else { 0 } } else { code };
    }

    // Multi-command pipeline
    let mut prev_read_fd: Option<RawFd> = None;
    let mut child_pids: Vec<Pid> = Vec::new();
    let mut pgid = Pid::from_raw(0); // will be set to first child's pid

    for (i, cmd) in cmds.iter().enumerate() {
        let is_last = i == cmds.len() - 1;

        // Create pipe for all but the last command
        let (read_fd, write_fd): (Option<RawFd>, Option<RawFd>) = if !is_last {
            let (r, w) = pipe().expect("pipe failed");
            (Some(r.into_raw_fd()), Some(w.into_raw_fd()))
        } else {
            (None, None)
        };

        match unsafe { fork() } {
            Ok(ForkResult::Child) => {
                signal::reset_child_signals();
                // All pipeline children share the first child's process group
                let my_pid = nix::unistd::getpid();
                let target_pgid = if pgid.as_raw() == 0 { my_pid } else { pgid };
                setpgid(my_pid, target_pgid).ok();

                // Set up stdin from previous pipe
                if let Some(fd) = prev_read_fd {
                    dup2(fd, 0).ok();
                    close(fd).ok();
                }
                // Set up stdout to next pipe
                if let Some(fd) = write_fd {
                    dup2(fd, 1).ok();
                    close(fd).ok();
                }
                // Close read end of current pipe in child
                if let Some(fd) = read_fd {
                    close(fd).ok();
                }

                let code = execute_command(cmd, state);
                std::process::exit(code);
            }
            Ok(ForkResult::Parent { child }) => {
                // First child becomes the process group leader
                if pgid.as_raw() == 0 {
                    pgid = child;
                }
                setpgid(child, pgid).ok();
                child_pids.push(child);
                // Close write end of current pipe in parent
                if let Some(fd) = write_fd {
                    close(fd).ok();
                }
                // Close previous read end
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

    // Give terminal foreground to the pipeline's process group
    let shell_pgid = nix::unistd::getpgrp();
    if state.interactive && pgid.as_raw() != 0 {
        tcsetpgrp(std::io::stdin(), pgid).ok();
    }

    // Wait for all children and collect PIPESTATUS
    let mut last_status = 0;
    let mut pipestatus = Vec::new();
    for pid in child_pids {
        match waitpid(pid, None) {
            Ok(WaitStatus::Exited(_, code)) => { pipestatus.push(code); last_status = code; }
            Ok(WaitStatus::Signaled(_, sig, _)) => { let c = 128 + sig as i32; pipestatus.push(c); last_status = c; }
            _ => { pipestatus.push(1); last_status = 1; }
        }
    }

    // Reclaim terminal foreground for the shell
    if state.interactive {
        tcsetpgrp(std::io::stdin(), shell_pgid).ok();
    }
    state.pipestatus = pipestatus.clone();

    // pipefail: return first non-zero exit code
    if state.shell_opts.pipefail {
        if let Some(&code) = pipestatus.iter().find(|&&c| c != 0) {
            last_status = code;
        }
    }

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

fn execute_simple(cmd: &SimpleCommand, state: &mut ShellState) -> i32 {
    // xtrace: print command before executing
    if state.shell_opts.xtrace && !cmd.words.is_empty() {
        let trace: Vec<String> = cmd.words.iter().map(|w| expand_word_to_string(w, state)).collect();
        eprintln!("+ {}", trace.join(" "));
    }

    // Handle assignments only (no command)
    if cmd.words.is_empty() {
        for assign in &cmd.assignments {
            let value = expand_word_to_string(&assign.value, state);
            state.set_var(&assign.name, &value);
        }
        return 0;
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
        let full_cmd = format!("{} {}", alias_val, args.join(" "));
        match crate::parser::parse(&full_cmd) {
            Ok(cmds) => {
                let mut last = 0;
                for c in &cmds {
                    last = execute_complete_command(c, state);
                }
                return last;
            }
            Err(_) => {} // fall through
        }
    }

    // Check for function
    if let Some(func_body) = state.functions.get(cmd_name).cloned() {
        state.push_positional_params(args.to_vec());
        let code = execute_compound(&func_body, state);
        state.pop_positional_params();
        return code;
    }

    // Check for builtin
    if builtins::is_builtin(cmd_name) {
        // Handle redirections for builtins
        let saved_fds = setup_redirects(&cmd.redirects, state);
        // Handle pre-command assignments
        let saved_vars: Vec<(String, Option<String>)> = cmd.assignments.iter().map(|a| {
            let old = state.get_var(&a.name).map(|s| s.to_string());
            let val = expand_word_to_string(&a.value, state);
            state.set_var(&a.name, &val);
            (a.name.clone(), old)
        }).collect();

        let code = builtins::run_builtin(cmd_name, &args.to_vec(), state);

        // Restore variables (assignments before builtins are temporary unless export)
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
    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            signal::reset_child_signals();
            let pid = nix::unistd::getpid();
            setpgid(pid, pid).ok();

            // Apply redirections
            apply_redirects_in_child(&cmd.redirects, state);

            // Set env vars from assignments
            for assign in &cmd.assignments {
                let val = expand_word_to_string(&assign.value, state);
                std::env::set_var(&assign.name, &val);
            }

            // Exec
            let c_cmd = CString::new(cmd_name.as_str()).unwrap_or_default();
            let c_args: Vec<CString> = expanded.iter()
                .map(|s| CString::new(s.as_str()).unwrap_or_default())
                .collect();

            let _ = execvp(&c_cmd, &c_args);
            eprintln!("rsh: {}: command not found", cmd_name);
            std::process::exit(127);
        }
        Ok(ForkResult::Parent { child }) => {
            // Put child in its own process group (race-free: both parent and child call setpgid)
            setpgid(child, child).ok();
            if state.interactive {
                wait_for_fg(child, state)
            } else {
                match waitpid(child, None) {
                    Ok(WaitStatus::Exited(_, code)) => code,
                    Ok(WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
                    _ => 1,
                }
            }
        }
        Err(e) => {
            eprintln!("rsh: fork failed: {}", e);
            1
        }
    }
}

fn execute_compound(cmd: &CompoundCommand, state: &mut ShellState) -> i32 {
    match cmd {
        CompoundCommand::BraceGroup { body, redirects } => {
            let saved = setup_redirects(redirects, state);
            let code = execute_command_list(body, state);
            restore_fds(saved);
            code
        }
        CompoundCommand::Subshell { body, redirects } => {
            match unsafe { fork() } {
                Ok(ForkResult::Child) => {
                    signal::reset_child_signals();
                    let pid = nix::unistd::getpid();
                    setpgid(pid, pid).ok();
                    apply_redirects_in_child(redirects, state);
                    let code = execute_command_list(body, state);
                    std::process::exit(code);
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
            let saved = setup_redirects(redirects, state);
            let mut code = 0;
            let mut matched = false;

            for (condition, body) in conditions {
                let cond_code = execute_command_list(condition, state);
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
            restore_fds(saved);
            code
        }
        CompoundCommand::For { var, words, body, redirects } => {
            let saved = setup_redirects(redirects, state);
            let word_list = match words {
                Some(ws) => expand_words(ws, state),
                None => state.positional_params.clone(),
            };

            let mut code = 0;
            for w in &word_list {
                state.set_var(var, w);
                code = execute_command_list(body, state);
            }
            restore_fds(saved);
            code
        }
        CompoundCommand::While { condition, body, redirects } => {
            let saved = setup_redirects(redirects, state);
            let mut code = 0;
            loop {
                let cond = execute_command_list(condition, state);
                if cond != 0 { break; }
                code = execute_command_list(body, state);
            }
            restore_fds(saved);
            code
        }
        CompoundCommand::Until { condition, body, redirects } => {
            let saved = setup_redirects(redirects, state);
            let mut code = 0;
            loop {
                let cond = execute_command_list(condition, state);
                if cond == 0 { break; }
                code = execute_command_list(body, state);
            }
            restore_fds(saved);
            code
        }
        CompoundCommand::Case { word, arms, redirects } => {
            let saved = setup_redirects(redirects, state);
            let value = expand_word_to_string(word, state);
            let mut code = 0;

            for arm in arms {
                for pattern in &arm.patterns {
                    let pat = expand_word_to_string(pattern, state);
                    if match_pattern(&value, &pat) {
                        code = execute_command_list(&arm.body, state);
                        restore_fds(saved);
                        return code;
                    }
                }
            }
            restore_fds(saved);
            code
        }
    }
}

fn execute_command_list(cmds: &[CompleteCommand], state: &mut ShellState) -> i32 {
    let mut code = 0;
    for cmd in cmds {
        code = execute_complete_command(cmd, state);
    }
    code
}

/// Glob-like pattern matching for case statements.
fn match_pattern(value: &str, pattern: &str) -> bool {
    crate::glob_match::glob_match(pattern, value)
}

// --- Redirect handling ---

struct SavedFd {
    original_fd: RawFd,
    saved_fd: RawFd,
}

/// Apply a single redirect: open the target and dup2 onto `fd`.
fn apply_one_redirect(kind: &RedirectKind, fd: RawFd, target_str: &str) {
    match kind {
        RedirectKind::Output => {
            if let Ok(file) = File::create(target_str) {
                dup2(file.into_raw_fd(), fd).ok();
            }
        }
        RedirectKind::Append => {
            if let Ok(file) = OpenOptions::new().create(true).append(true).open(target_str) {
                dup2(file.into_raw_fd(), fd).ok();
            }
        }
        RedirectKind::Input => {
            if let Ok(file) = File::open(target_str) {
                dup2(file.into_raw_fd(), fd).ok();
            }
        }
        RedirectKind::HereString | RedirectKind::HereDoc => {
            let (r, w) = pipe().expect("pipe");
            let r_fd = r.into_raw_fd();
            let w_fd = w.into_raw_fd();
            let data = format!("{}\n", target_str);
            unsafe { nix::libc::write(w_fd, data.as_ptr() as *const _, data.len()); }
            close(w_fd).ok();
            dup2(r_fd, fd).ok();
            close(r_fd).ok();
        }
        RedirectKind::DupOutput => {
            if let Ok(target_fd) = target_str.parse::<RawFd>() {
                dup2(target_fd, fd).ok();
            }
        }
        _ => {}
    }
}

fn redirect_fd(redir: &Redirect) -> RawFd {
    match redir.kind {
        RedirectKind::Output | RedirectKind::Append | RedirectKind::DupOutput => redir.fd.unwrap_or(1),
        RedirectKind::Input | RedirectKind::HereString | RedirectKind::HereDoc => redir.fd.unwrap_or(0),
        _ => redir.fd.unwrap_or(1),
    }
}

fn setup_redirects(redirects: &[Redirect], state: &mut ShellState) -> Vec<SavedFd> {
    let mut saved = Vec::new();
    for redir in redirects {
        let target_str = expand_word_to_string(&redir.target, state);
        let fd = redirect_fd(redir);

        // Save original fd
        if let Ok(sfd) = nix::unistd::dup(fd) {
            saved.push(SavedFd { original_fd: fd, saved_fd: sfd });
        }

        apply_one_redirect(&redir.kind, fd, &target_str);
    }
    saved
}

fn restore_fds(saved: Vec<SavedFd>) {
    for s in saved.into_iter().rev() {
        dup2(s.saved_fd, s.original_fd).ok();
        close(s.saved_fd).ok();
    }
}

fn apply_redirects_in_child(redirects: &[Redirect], state: &mut ShellState) {
    for redir in redirects {
        let target_str = expand_word_to_string(&redir.target, state);
        let fd = redirect_fd(redir);
        apply_one_redirect(&redir.kind, fd, &target_str);
    }
}
