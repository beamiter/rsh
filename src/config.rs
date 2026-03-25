/// Config file loading: source ~/.rshrc on startup.

use crate::environment::ShellState;
use crate::executor;
use crate::parser;
use std::path::PathBuf;

pub fn load_config(state: &mut ShellState) {
    let rshrc = state.home_dir.join(".rshrc");
    if rshrc.exists() {
        source_file(&rshrc, state);
    }
}

fn source_file(path: &PathBuf, state: &mut ShellState) {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            match parser::parse(&content) {
                Ok(commands) => {
                    for cmd in &commands {
                        executor::execute_complete_command(cmd, state);
                    }
                }
                Err(e) => {
                    eprintln!("rsh: error in {}: {}", path.display(), e);
                }
            }
        }
        Err(_) => {} // Silent if can't read
    }
}
