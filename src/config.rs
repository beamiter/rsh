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

/// Load bash file with lenient error handling - use bash as fallback for complex scripts
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
                    // Full parse failed, use bash as fallback for complex scripts
                    source_via_bash_fallback(path, state);
                }
            }
        }
        Err(_) => {} // Silent if can't read
    }
}

/// Use bash to source a script file and reload environment variables
fn source_via_bash_fallback(path: &PathBuf, state: &mut ShellState) {
    let path_str = path.to_string_lossy().to_string();
    let bash_script = format!(
        r#"
set -a
source "{path}"
set +a

# Output all environment variables in key=value format
declare -p | grep 'declare -x' | sed 's/declare -x //' | sed "s/='/'=/g"
"#,
        path = path_str.replace("'", "\\'")
    );

    // Execute bash script to capture the environment
    if let Ok(output) = std::process::Command::new("bash")
        .arg("-c")
        .arg(&bash_script)
        .output() {
        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse exported variables from bash output
        for line in stdout.lines() {
            // Skip function names (no = sign)
            if line.contains('=') {
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
    }
}

