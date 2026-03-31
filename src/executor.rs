/// AST executor: fork/exec, pipes, redirects, compound commands.

use crate::builtins;
use crate::environment::ShellState;
use crate::expand::{expand_word_to_string, expand_words};
use crate::parser::ast::*;
use crate::signal;

use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{close, execvp, fork, pipe, setpgid, tcsetpgrp, ForkResult, Pid};
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::os::unix::io::{IntoRawFd, RawFd, AsRawFd, OwnedFd, BorrowedFd};

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

pub fn execute_program(commands: &[CompleteCommand], state: &mut ShellState) -> i32 {
    let mut last = 0;
    for cmd in commands {
        last = execute_complete_command(cmd, state);
    }
    last
}

pub fn execute_complete_command(cmd: &CompleteCommand, state: &mut ShellState) -> i32 {
    if cmd.background {
        // Fork for background execution
        match unsafe { fork() } {
            Ok(ForkResult::Child) => {
                signal::reset_child_signals();
                let pid = nix::unistd::getpid();
                setpgid(pid, pid).ok();
                let code = execute_and_or(&cmd.list, state);
                std::process::exit(code);
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

fn execute_pipeline(pipeline: &Pipeline, state: &mut ShellState) -> i32 {
    let cmds = &pipeline.commands;

    if cmds.len() == 1 {
        let code = execute_command(&cmds[0], state);
        return if pipeline.negated { if code == 0 { 1 } else { 0 } } else { code };
    }

    let mut prev_read_fd: Option<RawFd> = None;
    let mut child_pids: Vec<Pid> = Vec::new();
    let mut pgid = Pid::from_raw(0);

    for (i, cmd) in cmds.iter().enumerate() {
        let is_last = i == cmds.len() - 1;

        let (read_fd, write_fd): (Option<RawFd>, Option<RawFd>) = if !is_last {
            let (r, w) = pipe().expect("pipe failed");
            (Some(r.into_raw_fd()), Some(w.into_raw_fd()))
        } else {
            (None, None)
        };

        match unsafe { fork() } {
            Ok(ForkResult::Child) => {
                signal::reset_child_signals();
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

                let code = execute_command(cmd, state);
                std::process::exit(code);
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
    state.pipestatus = pipestatus;

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

fn execute_simple(cmd: &SimpleCommand, state: &mut ShellState) -> i32 {
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
            Err(_) => {}
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
            eprintln!("rsh: {}: command not found", cmd_name);
            std::process::exit(127);
        }
        Ok(ForkResult::Parent { child }) => {
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

pub fn execute_compound(cmd: &CompoundCommand, state: &mut ShellState) -> i32 {
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

fn apply_one_redirect(kind: &RedirectKind, fd: RawFd, target_str: &str) {
    match kind {
        RedirectKind::Output => {
            if let Ok(file) = File::create(target_str) {
                let src = file.into_raw_fd();
                dup2_raw(src, fd).ok();
                if src != fd { close(src).ok(); }
            }
        }
        RedirectKind::Append => {
            if let Ok(file) = OpenOptions::new().create(true).append(true).open(target_str) {
                let src = file.into_raw_fd();
                dup2_raw(src, fd).ok();
                if src != fd { close(src).ok(); }
            }
        }
        RedirectKind::Input => {
            if let Ok(file) = File::open(target_str) {
                let src = file.into_raw_fd();
                dup2_raw(src, fd).ok();
                if src != fd { close(src).ok(); }
            }
        }
        RedirectKind::HereString | RedirectKind::HereDoc => {
            let (r, w) = pipe().expect("pipe");
            let r_fd = r.into_raw_fd();
            let w_fd = w.into_raw_fd();
            let data = format!("{}\n", target_str);
            unsafe { nix::libc::write(w_fd, data.as_ptr() as *const _, data.len()); }
            close(w_fd).ok();
            dup2_raw(r_fd, fd).ok();
            close(r_fd).ok();
        }
        RedirectKind::DupOutput => {
            if let Ok(target_fd) = target_str.parse::<RawFd>() {
                dup2_raw(target_fd, fd).ok();
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

        // Safe because fd is a valid file descriptor at this point
        unsafe {
            if let Ok(sfd) = nix::unistd::dup(BorrowedFd::borrow_raw(fd)) {
                saved.push(SavedFd { original_fd: fd, saved_fd: sfd });
            }
        }

        apply_one_redirect(&redir.kind, fd, &target_str);
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
        let target_str = expand_word_to_string(&redir.target, state);
        let fd = redirect_fd(redir);
        apply_one_redirect(&redir.kind, fd, &target_str);
    }
}
