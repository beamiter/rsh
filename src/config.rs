/// Config file loading: source ~/.bashrc on startup.

use crate::environment::ShellState;
use crate::executor;
use crate::parser;
use std::path::PathBuf;

pub fn load_config(state: &mut ShellState) {
    // Load ~/.bashrc (bash-compatible startup configuration)
    let bashrc = state.home_dir.join(".bashrc");
    if bashrc.exists() {
        source_file_lenient(&bashrc, state);
    }
}

/// Load bash file with lenient error handling - skip unparseable lines
fn source_file_lenient(path: &PathBuf, state: &mut ShellState) {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            // Try parsing entire file first
            match parser::parse(&content) {
                Ok(commands) => {
                    // Full parse succeeded, execute all commands
                    for cmd in &commands {
                        executor::execute_complete_command(cmd, state);
                    }
                }
                Err(_) => {
                    // Full parse failed, try parsing line by line
                    eprintln!("rsh: {} has some unsupported bash syntax, loading available parts...", path.display());
                    for (_line_no, line) in content.lines().enumerate() {
                        let trimmed = line.trim();

                        // Skip empty lines and comments
                        if trimmed.is_empty() || trimmed.starts_with('#') {
                            continue;
                        }

                        // Try to parse this line
                        match parser::parse(trimmed) {
                            Ok(commands) => {
                                for cmd in &commands {
                                    executor::execute_complete_command(cmd, state);
                                }
                            }
                            Err(_) => {
                                // Silently skip unparseable lines
                                // eprintln!("rsh: {} line {}: skipped due to syntax", path.display(), line_no + 1);
                            }
                        }
                    }
                }
            }
        }
        Err(_) => {} // Silent if can't read
    }
}

