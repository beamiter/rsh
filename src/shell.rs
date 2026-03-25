/// Main shell REPL loop.

use crate::config;
use crate::editor::Editor;
use crate::environment::ShellState;
use crate::executor;
use crate::history::History;
use crate::parser;
use crate::signal;

pub struct Shell {
    pub state: ShellState,
    pub history: History,
    pub editor: Editor,
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

        // Check for -c flag or script argument
        let args: Vec<String> = std::env::args().collect();

        // Only load config for interactive mode
        let is_cmd_mode = args.len() >= 3 && args[1] == "-c";
        let is_script_mode = args.len() >= 2 && !args[1].starts_with('-');
        if !is_cmd_mode && !is_script_mode {
            config::load_config(&mut self.state);
        }
        if args.len() >= 3 && args[1] == "-c" {
            // Execute command string: rsh -c 'command'
            let cmd_str = &args[2];
            if args.len() > 3 {
                self.state.positional_params = args[3..].to_vec();
            }
            match parser::parse(cmd_str) {
                Ok(commands) => {
                    for cmd in &commands {
                        let code = executor::execute_complete_command(cmd, &mut self.state);
                        self.state.last_exit_code = code;
                    }
                }
                Err(e) => {
                    eprintln!("rsh: {}", e);
                    self.state.last_exit_code = 2;
                }
            }
            // Run EXIT trap
            self.run_exit_trap();
            std::process::exit(self.state.last_exit_code);
        }

        if args.len() >= 2 && !args[1].starts_with('-') {
            // Script mode: rsh script.sh [args...]
            let script = &args[1];
            if args.len() > 2 {
                self.state.positional_params = args[2..].to_vec();
            }
            self.state.interactive = false;
            match std::fs::read_to_string(script) {
                Ok(content) => {
                    // Skip shebang line
                    let content = if content.starts_with("#!") {
                        content.splitn(2, '\n').nth(1).unwrap_or("").to_string()
                    } else {
                        content
                    };
                    match parser::parse(&content) {
                        Ok(commands) => {
                            for cmd in &commands {
                                let code = executor::execute_complete_command(cmd, &mut self.state);
                                self.state.last_exit_code = code;
                                if self.state.shell_opts.errexit && code != 0 {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("rsh: {}: {}", script, e);
                            self.state.last_exit_code = 2;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("rsh: {}: {}", script, e);
                    self.state.last_exit_code = 127;
                }
            }
            self.run_exit_trap();
            std::process::exit(self.state.last_exit_code);
        }

        // Interactive mode - main loop
        loop {
            // Check background jobs
            self.state.jobs.check_background();
            self.state.jobs.notify_done();

            match self.editor.read_line(&mut self.state, &mut self.history) {
                Ok(Some(line)) => {
                    let line = line.trim().to_string();
                    if line.is_empty() { continue; }

                    // Add to history
                    self.history.add(&line);

                    // Parse and execute
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
                }
                Ok(None) => {
                    // EOF (Ctrl-D)
                    break;
                }
                Err(e) => {
                    eprintln!("rsh: editor error: {}", e);
                }
            }
        }

        self.run_exit_trap();
        self.history.save();
    }

    fn run_exit_trap(&mut self) {
        if let Some(cmd) = self.state.traps.get("EXIT").cloned() {
            if let Ok(commands) = parser::parse(&cmd) {
                for c in &commands {
                    executor::execute_complete_command(c, &mut self.state);
                }
            }
        }
    }
}
