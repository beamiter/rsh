/// Config file loading: source ~/.rshrc and ~/.bashrc on startup.

use crate::environment::ShellState;
use crate::executor;
use crate::parser;
use std::path::PathBuf;

pub fn load_config(state: &mut ShellState) {
    // Load ~/.rshrc first (rsh-specific config)
    let rshrc = state.home_dir.join(".rshrc");
    if rshrc.exists() {
        source_file(&rshrc, state, false);
    }

    // Load ~/.bashrc (bash compatibility)
    let bashrc = state.home_dir.join(".bashrc");
    if bashrc.exists() {
        source_file(&bashrc, state, true);
    }
}

fn source_file(path: &PathBuf, state: &mut ShellState, is_bashrc: bool) {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            match parser::parse(&content) {
                Ok(commands) => {
                    for cmd in &commands {
                        let result = executor::execute_complete_command(cmd, state);
                        // For .bashrc, warn but continue on command errors
                        // For .rshrc, allow normal error handling
                        if is_bashrc && result != 0 {
                            // silently continue - bash .bashrc should be lenient
                        }
                    }
                }
                Err(e) => {
                    if is_bashrc {
                        // Warning mode for .bashrc - don't fail the entire shell
                        eprintln!("rsh: warning: {} contains unsupported syntax: {}", path.display(), e);
                    } else {
                        eprintln!("rsh: error in {}: {}", path.display(), e);
                    }
                }
            }
        }
        Err(_) => {} // Silent if can't read
    }
}
