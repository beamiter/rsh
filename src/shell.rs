/// Main shell REPL loop.

use crate::config;
use crate::editor::Editor;
use crate::environment::ShellState;
use crate::executor;
use crate::history::History;
use crate::job::JobTable;
use crate::parser;
use crate::signal;

pub struct Shell {
    pub state: ShellState,
    pub history: History,
    pub jobs: JobTable,
    pub editor: Editor,
}

impl Shell {
    pub fn new() -> Self {
        Shell {
            state: ShellState::new(true),
            history: History::new(10000),
            jobs: JobTable::new(),
            editor: Editor::new(),
        }
    }

    pub fn run(&mut self) {
        signal::install_shell_signals();

        // Load config
        config::load_config(&mut self.state);

        // Main loop
        loop {
            // Check background jobs
            self.jobs.check_background();
            self.jobs.notify_done();

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

        self.history.save();
    }
}
