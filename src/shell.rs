/// Main shell REPL loop.

use crate::config;
use crate::editor::Editor;
use crate::environment::ShellState;
use crate::executor;
use crate::history::History;
use crate::hooks;
use crate::parser;
use crate::signal;

pub struct Shell {
    pub state: ShellState,
    pub history: History,
    pub editor: Editor,
}

/// Run non-interactive modes (-c command, script file) without creating
/// Editor or History, avoiding fork overhead from `tput` / terminal queries.
pub fn run_noninteractive() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() >= 3 && args[1] == "-c" {
        let mut state = ShellState::new(false);
        signal::install_shell_signals();
        let cmd_str = &args[2];
        if args.len() > 3 {
            state.positional_params = args[3..].to_vec();
        }
        match parser::parse(cmd_str) {
            Ok(commands) => {
                for cmd in &commands {
                    let code = executor::execute_complete_command(cmd, &mut state);
                    state.last_exit_code = code;
                }
            }
            Err(e) => {
                eprintln!("rsh: {}", e);
                state.last_exit_code = 2;
            }
        }
        run_exit_trap(&mut state);
        return Some(state.last_exit_code);
    }

    if args.len() >= 2 && !args[1].starts_with('-') {
        let mut state = ShellState::new(false);
        signal::install_shell_signals();
        let script = &args[1];
        if args.len() > 2 {
            state.positional_params = args[2..].to_vec();
        }
        match std::fs::read_to_string(script) {
            Ok(content) => {
                let content = if content.starts_with("#!") {
                    content.splitn(2, '\n').nth(1).unwrap_or("").to_string()
                } else {
                    content
                };
                match parser::parse(&content) {
                    Ok(commands) => {
                        for cmd in &commands {
                            let code = executor::execute_complete_command(cmd, &mut state);
                            state.last_exit_code = code;
                            if state.shell_opts.errexit && code != 0 {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("rsh: {}: {}", script, e);
                        state.last_exit_code = 2;
                    }
                }
            }
            Err(e) => {
                eprintln!("rsh: {}: {}", script, e);
                state.last_exit_code = 127;
            }
        }
        run_exit_trap(&mut state);
        return Some(state.last_exit_code);
    }

    None // interactive mode
}

/// Expand history references: !!, !$, !n, !-n
fn expand_history(line: &str, history: &crate::history::History) -> Option<String> {
    if !line.contains('!') {
        return Some(line.to_string());
    }

    let mut result = String::new();
    let mut chars = line.chars().peekable();
    let mut in_single_quote = false;

    while let Some(c) = chars.next() {
        if c == '\'' {
            in_single_quote = !in_single_quote;
            result.push(c);
            continue;
        }
        if in_single_quote || c != '!' {
            result.push(c);
            continue;
        }
        match chars.peek() {
            Some(&'!') => {
                chars.next();
                result.push_str(history.last()?);
            }
            Some(&'$') => {
                chars.next();
                let prev = history.last()?;
                let last_arg = prev.split_whitespace().last().unwrap_or(prev);
                result.push_str(last_arg);
            }
            Some(&c2) if c2.is_ascii_digit() => {
                let mut num_str = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() {
                        num_str.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let n: usize = num_str.parse().unwrap_or(0);
                if n == 0 { return None; }
                result.push_str(history.get(n - 1)?);
            }
            Some(&'-') => {
                chars.next();
                let mut num_str = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() {
                        num_str.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let n: usize = num_str.parse().unwrap_or(0);
                if n == 0 || n > history.len() { return None; }
                result.push_str(history.get(history.len() - n)?);
            }
            _ => {
                result.push('!');
            }
        }
    }
    Some(result)
}

fn run_exit_trap(state: &mut ShellState) {
    if let Some(cmd) = state.traps.get("EXIT").cloned() {
        if let Ok(commands) = parser::parse(&cmd) {
            for c in &commands {
                executor::execute_complete_command(c, state);
            }
        }
    }
}

impl Shell {
    pub fn new() -> Self {
        Shell {
            state: ShellState::new(true),
            history: History::new(10000),
            editor: Editor::new(),
        }
    }

    pub fn run(&mut self) {
        signal::install_shell_signals();
        config::load_config(&mut self.state);

        loop {
            // Check background jobs
            self.state.jobs.check_background();
            self.state.jobs.notify_done_with_notification(self.state.notification_threshold);

            // Run precmd hooks
            let precmd = self.state.hooks.precmd.clone();
            hooks::run_hooks(&precmd, &mut self.state);

            match self.editor.read_line(&mut self.state, &mut self.history) {
                Ok(Some(line)) => {
                    let line = line.trim().to_string();
                    if line.is_empty() { continue; }

                    // History expansion
                    let line = match expand_history(&line, &self.history) {
                        Some(expanded) => {
                            if expanded != line {
                                eprintln!("{}", expanded);
                            }
                            expanded
                        }
                        None => {
                            eprintln!("rsh: !: event not found");
                            continue;
                        }
                    };

                    self.history.add(&line);

                    // Run preexec hooks
                    let preexec = self.state.hooks.preexec.clone();
                    hooks::run_hooks(&preexec, &mut self.state);

                    // Parse and execute
                    let cmd_start = std::time::Instant::now();
                    match parser::parse(&line) {
                        Ok(commands) => {
                            for cmd in &commands {
                                let code = executor::execute_complete_command(cmd, &mut self.state);
                                self.state.last_exit_code = code;
                                if self.state.shell_opts.errexit && code != 0 {
                                    eprintln!("rsh: errexit: command exited with status {}", code);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("rsh: {}", e);
                            self.state.last_exit_code = 2;
                        }
                    }
                    self.state.last_command_duration = Some(cmd_start.elapsed());
                }
                Ok(None) => {
                    break;
                }
                Err(e) => {
                    eprintln!("rsh: editor error: {}", e);
                }
            }
        }

        run_exit_trap(&mut self.state);
        self.history.save();
    }
}
