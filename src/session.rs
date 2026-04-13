/// Session persistence: save/restore shell state across terminal restarts.
///
/// When jterm4 spawns rsh with `--session <id>`, rsh restores state from
/// `~/.rsh/sessions/<id>.json`. On exit, rsh saves a snapshot back.

use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::environment::*;
use crate::parser::ast::CompoundCommand;

/// Snapshot format version. Bump when adding fields (use #[serde(default)] for compat).
const SNAPSHOT_VERSION: u32 = 1;

/// Environment variables that should NOT be persisted across sessions.
/// These are process-specific or terminal-specific and would be stale after restore.
const SKIP_ENV_VARS: &[&str] = &[
    // Process-specific
    "BASHPID", "PPID", "SHLVL", "_", "OLDPWD",
    // Terminal-specific (re-set by the new terminal)
    "COLUMNS", "LINES", "TERM", "COLORTERM",
    "WINDOWID", "DISPLAY", "WAYLAND_DISPLAY",
    // Session-specific
    "SSH_AUTH_SOCK", "SSH_AGENT_PID",
    "GPG_AGENT_INFO", "DBUS_SESSION_BUS_ADDRESS",
    "XDG_SESSION_ID", "XDG_RUNTIME_DIR",
    // Internal
    "RSH_SESSION_ID", "TERM_SESSION_ID",
];

/// Detected environment context for re-activation on restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EnvironmentContext {
    Plain,
    PythonVenv { virtual_env: String },
    NixShell {
        #[serde(default)]
        flake_dir: Option<String>,
        nix_build_top: Option<String>,
    },
    Docker { container_id: Option<String> },
    Ssh { ssh_connection: String },
}

impl Default for EnvironmentContext {
    fn default() -> Self {
        EnvironmentContext::Plain
    }
}

/// Serializable snapshot of shell session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub version: u32,
    pub session_id: String,
    pub cwd: String,
    pub env_vars: HashMap<String, String>,
    pub aliases: HashMap<String, String>,
    pub functions: HashMap<String, CompoundCommand>,
    pub arrays: HashMap<String, Vec<String>>,
    pub assoc_arrays: HashMap<String, HashMap<String, String>>,
    pub shell_opts: ShellOpts,
    pub hooks: ShellHooks,
    pub traps: HashMap<String, String>,
    pub completion_specs: HashMap<String, CompletionSpec>,
    pub dir_stack: Vec<PathBuf>,
    pub editing_mode: EditingMode,
    pub prompt_style: PromptStyle,
    pub last_exit_code: i32,
    pub notification_threshold_secs: u64,
    #[serde(default)]
    pub environment_context: EnvironmentContext,
}

/// Directory where session snapshot files are stored.
fn sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".rsh")
        .join("sessions")
}

/// Full path for a session snapshot file.
fn session_file(session_id: &str) -> PathBuf {
    // Sanitize session_id to prevent path traversal
    let safe_id: String = session_id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    sessions_dir().join(format!("{}.json", safe_id))
}

impl SessionSnapshot {
    /// Capture the current shell state into a serializable snapshot.
    pub fn capture(state: &ShellState, session_id: &str) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "/".to_string());

        // Filter out process/terminal-specific env vars
        let env_vars: HashMap<String, String> = state.env_vars.iter()
            .filter(|(k, _)| !SKIP_ENV_VARS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        SessionSnapshot {
            version: SNAPSHOT_VERSION,
            session_id: session_id.to_string(),
            cwd,
            env_vars,
            aliases: state.aliases.clone(),
            functions: state.functions.clone(),
            arrays: state.arrays.clone(),
            assoc_arrays: state.assoc_arrays.clone(),
            shell_opts: state.shell_opts.clone(),
            hooks: state.hooks.clone(),
            traps: state.traps.clone(),
            completion_specs: state.completion_specs.clone(),
            dir_stack: state.dir_stack.clone(),
            editing_mode: state.editing_mode.clone(),
            prompt_style: state.prompt_style,
            last_exit_code: state.last_exit_code,
            notification_threshold_secs: state.notification_threshold.as_secs(),
            environment_context: detect_environment(),
        }
    }

    /// Apply this snapshot to a ShellState, restoring its fields.
    pub fn apply(self, state: &mut ShellState) {
        // Restore CWD
        if let Err(e) = std::env::set_current_dir(&self.cwd) {
            eprintln!("rsh: session restore: failed to cd to {}: {}", self.cwd, e);
        }

        // Merge env vars: snapshot values override, but keep process-inherited vars for SKIP list
        for (k, v) in &self.env_vars {
            state.env_vars.insert(k.clone(), v.clone());
            std::env::set_var(k, v);
        }

        state.aliases = self.aliases;
        state.functions = self.functions;
        state.arrays = self.arrays;
        state.assoc_arrays = self.assoc_arrays;
        state.shell_opts = self.shell_opts;
        state.hooks = self.hooks;
        state.traps = self.traps;
        state.completion_specs = self.completion_specs;
        state.dir_stack = self.dir_stack;
        state.editing_mode = self.editing_mode;
        state.prompt_style = self.prompt_style;
        state.last_exit_code = self.last_exit_code;
        state.notification_threshold = std::time::Duration::from_secs(self.notification_threshold_secs);
    }

    /// Save snapshot to disk as JSON (atomic write).
    pub fn save(&self) -> Result<(), std::io::Error> {
        let dir = sessions_dir();
        std::fs::create_dir_all(&dir)?;

        let path = session_file(&self.session_id);
        let tmp_path = path.with_extension("json.tmp");

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        std::fs::write(&tmp_path, &json)?;

        // Atomic rename
        if let Err(e) = std::fs::rename(&tmp_path, &path) {
            let _ = std::fs::remove_file(&path);
            if let Err(e2) = std::fs::rename(&tmp_path, &path) {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("failed to rename session file: {} / {}", e, e2),
                ));
            }
        }

        Ok(())
    }

    /// Load a snapshot from disk by session ID.
    pub fn load(session_id: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let path = session_file(session_id);
        let json = std::fs::read_to_string(&path)?;
        let snapshot: SessionSnapshot = serde_json::from_str(&json)?;
        Ok(snapshot)
    }

    /// Delete the session file (consume-on-start).
    pub fn delete(session_id: &str) {
        let path = session_file(session_id);
        let _ = std::fs::remove_file(&path);
    }
}

/// Search for `flake.nix` starting from CWD and walking up to parent directories.
/// Returns the directory containing the flake, or None.
fn find_flake_dir() -> Option<String> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("flake.nix").exists() {
            return Some(dir.to_string_lossy().to_string());
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Detect the current environment context by checking env vars and filesystem markers.
pub fn detect_environment() -> EnvironmentContext {
    // Python venv
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        if !venv.is_empty() {
            return EnvironmentContext::PythonVenv { virtual_env: venv };
        }
    }

    // Nix shell — check env var first (rsh is the nix shell itself),
    // then check for flake.nix (rsh is the parent, nix develop ran as child)
    let in_nix = std::env::var("IN_NIX_SHELL").is_ok() || std::env::var("NIX_BUILD_TOP").is_ok();
    let flake_dir = find_flake_dir();
    if in_nix || flake_dir.is_some() {
        return EnvironmentContext::NixShell {
            flake_dir,
            nix_build_top: std::env::var("NIX_BUILD_TOP").ok(),
        };
    }

    // Docker
    if std::path::Path::new("/.dockerenv").exists() || std::env::var("DOCKER_CONTAINER").is_ok() {
        let container_id = std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string());
        return EnvironmentContext::Docker { container_id };
    }

    // SSH
    if let Ok(conn) = std::env::var("SSH_CONNECTION") {
        if !conn.is_empty() {
            return EnvironmentContext::Ssh { ssh_connection: conn };
        }
    }
    if std::env::var("SSH_CLIENT").is_ok() {
        let conn = std::env::var("SSH_CLIENT").unwrap_or_default();
        return EnvironmentContext::Ssh { ssh_connection: conn };
    }

    EnvironmentContext::Plain
}

/// Re-activate environment context after restoring a session.
pub fn reactivate_environment(ctx: &EnvironmentContext, state: &mut ShellState) {
    match ctx {
        EnvironmentContext::PythonVenv { virtual_env } => {
            let venv_path = std::path::Path::new(virtual_env);
            let activate = venv_path.join("bin").join("activate");
            if activate.exists() {
                // Set VIRTUAL_ENV and prepend its bin to PATH
                state.export_var("VIRTUAL_ENV", virtual_env);
                let venv_bin = venv_path.join("bin");
                if let Some(path) = state.env_vars.get("PATH").cloned() {
                    let venv_bin_str = venv_bin.to_string_lossy();
                    // Only prepend if not already there
                    if !path.split(':').any(|p| p == venv_bin_str.as_ref()) {
                        let new_path = format!("{}:{}", venv_bin_str, path);
                        state.export_var("PATH", &new_path);
                    }
                }
            } else {
                eprintln!("rsh: session restore: venv {} no longer exists", virtual_env);
            }
        }
        EnvironmentContext::NixShell { flake_dir, .. } => {
            reactivate_nix_develop(flake_dir.as_deref(), state);
        }
        EnvironmentContext::Docker { .. } | EnvironmentContext::Ssh { .. } => {
            // Docker/SSH context is informational at the rsh level.
            // Re-establishing the connection is jterm4's responsibility.
        }
        EnvironmentContext::Plain => {}
    }
}

/// Re-activate a nix develop environment by running `nix print-dev-env`
/// and applying the resulting environment variables to the shell state.
fn reactivate_nix_develop(flake_dir: Option<&str>, state: &mut ShellState) {
    let Some(dir) = flake_dir else { return };

    // Check that flake.nix still exists
    let flake_path = std::path::Path::new(dir).join("flake.nix");
    if !flake_path.exists() {
        eprintln!("rsh: session restore: flake.nix no longer exists in {}", dir);
        return;
    }

    eprintln!("rsh: restoring nix develop environment from {} ...", dir);

    // Use `nix print-dev-env` to get the dev shell environment, then
    // eval it in bash and extract the resulting env vars.
    // This is the same pattern as config.rs::source_via_bash.
    let bash_script = format!(
        r#"eval "$(nix print-dev-env '{dir}' 2>/dev/null)" 2>/dev/null
echo "=== NIX_ENV ==="
env -0 2>/dev/null || env
"#,
        dir = dir.replace('\'', "'\\''")
    );

    let output = match std::process::Command::new("bash")
        .arg("-c")
        .arg(&bash_script)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("rsh: session restore: failed to run nix print-dev-env: {}", e);
            return;
        }
    };

    if !output.status.success() {
        eprintln!("rsh: session restore: nix print-dev-env failed (exit {})",
            output.status.code().unwrap_or(-1));
        return;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Find the NIX_ENV section
    let Some(env_section) = stdout.split("=== NIX_ENV ===\n").nth(1) else {
        eprintln!("rsh: session restore: could not parse nix environment output");
        return;
    };

    // Parse env vars — try NUL-separated first (from env -0), fall back to newline
    let entries: Vec<&str> = if env_section.contains('\0') {
        env_section.split('\0').collect()
    } else {
        env_section.lines().collect()
    };

    let mut count = 0;
    for entry in entries {
        if entry.is_empty() { continue; }
        if let Some(eq_pos) = entry.find('=') {
            let key = &entry[..eq_pos];
            let value = &entry[eq_pos + 1..];
            // Skip process-specific vars and vars we already filter
            if SKIP_ENV_VARS.contains(&key) { continue; }
            // Skip shell internals
            if key.starts_with("BASH") || key == "SHELLOPTS" || key == "IFS" { continue; }
            state.export_var(key, value);
            count += 1;
        }
    }

    if count > 0 {
        eprintln!("rsh: restored nix develop environment ({} vars)", count);
    }
}

/// Clean up session files older than max_age.
pub fn cleanup_stale_sessions(max_age: std::time::Duration) {
    let dir = sessions_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(metadata) = path.metadata() {
            if let Ok(modified) = metadata.modified() {
                if let Ok(age) = modified.elapsed() {
                    if age > max_age {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_snapshot_roundtrip() {
        let mut state = ShellState::new(false);
        state.aliases.insert("ll".to_string(), "ls -la".to_string());
        state.export_var("MY_VAR", "hello");
        state.shell_opts.extglob = true;
        state.hooks.precmd.push("echo hi".to_string());
        state.traps.insert("EXIT".to_string(), "echo bye".to_string());

        let snapshot = SessionSnapshot::capture(&state, "test-roundtrip");
        let json = serde_json::to_string_pretty(&snapshot).expect("serialize");
        let restored: SessionSnapshot = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored.session_id, "test-roundtrip");
        assert_eq!(restored.aliases.get("ll"), Some(&"ls -la".to_string()));
        assert_eq!(restored.env_vars.get("MY_VAR"), Some(&"hello".to_string()));
        assert_eq!(restored.shell_opts.extglob, true);
        assert_eq!(restored.hooks.precmd, vec!["echo hi".to_string()]);
        assert_eq!(restored.traps.get("EXIT"), Some(&"echo bye".to_string()));
    }

    #[test]
    fn test_env_var_filtering() {
        let mut state = ShellState::new(false);
        state.env_vars.insert("COLUMNS".to_string(), "80".to_string());
        state.env_vars.insert("LINES".to_string(), "24".to_string());
        state.env_vars.insert("MY_APP".to_string(), "value".to_string());

        let snapshot = SessionSnapshot::capture(&state, "test-filter");
        assert!(!snapshot.env_vars.contains_key("COLUMNS"));
        assert!(!snapshot.env_vars.contains_key("LINES"));
        assert_eq!(snapshot.env_vars.get("MY_APP"), Some(&"value".to_string()));
    }

    #[test]
    fn test_session_file_path_sanitization() {
        let path = session_file("../../../etc/passwd");
        assert!(!path.to_string_lossy().contains(".."));
        assert!(path.to_string_lossy().contains("etcpasswd"));
    }

    #[test]
    fn test_detect_environment_plain() {
        // In test context, typically no venv/nix/docker/ssh
        // Just verify it doesn't panic
        let ctx = detect_environment();
        // ctx could be anything depending on test environment
        match ctx {
            EnvironmentContext::Plain
            | EnvironmentContext::PythonVenv { .. }
            | EnvironmentContext::NixShell { .. }
            | EnvironmentContext::Docker { .. }
            | EnvironmentContext::Ssh { .. } => {}
        }
    }
}
