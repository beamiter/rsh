use std::collections::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;

use serde::{Serialize, Deserialize};

use crate::job::JobTable;
use crate::parser::ast::CompoundCommand;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EditingMode {
    Emacs,
    Vi,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConfigSource {
    Bashrc,    // 使用 .bashrc，直接用 bash 执行
    Rshrc,     // 使用 .rshrc，用 rsh 解析器执行
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptStyle {
    Full,     // user@host ~/path (branch) took duration ❯
    Compact,  // user ~/path (branch) ❯
    Minimal,  // ~/path ❯
    Auto,     // Automatically choose based on terminal width
}

impl Default for PromptStyle {
    fn default() -> Self {
        PromptStyle::Auto
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellOpts {
    pub errexit: bool,        // set -e
    pub xtrace: bool,         // set -x
    pub pipefail: bool,       // set -o pipefail
    pub globstar: bool,       // set -o globstar
    pub dotglob: bool,        // shopt dotglob: match hidden files
    pub nullglob: bool,       // shopt nullglob: empty string for no matches
    pub failglob: bool,       // shopt failglob: error on no matches
    pub extglob: bool,        // shopt extglob: extended glob patterns
    pub nocaseglob: bool,     // shopt nocaseglob: case-insensitive matching
    pub noglob: bool,         // shopt noglob: disable pathname expansion
    pub lastpipe: bool,       // shopt lastpipe: last pipe component in current shell
    pub autocd: bool,         // shopt autocd: cd to bare directory names
    pub cdspell: bool,        // shopt cdspell: correct cd spelling errors
    pub checkwinsize: bool,   // shopt checkwinsize: update LINES/COLUMNS
    pub inherit_errexit: bool,// shopt inherit_errexit: subshells inherit errexit
    pub config_source: ConfigSource, // which config file to use: .bashrc or .rshrc
}

impl Default for ShellOpts {
    fn default() -> Self {
        ShellOpts {
            errexit: false,
            xtrace: false,
            pipefail: false,
            globstar: true,
            dotglob: false,
            nullglob: false,
            failglob: false,
            extglob: false,
            nocaseglob: false,
            noglob: false,
            lastpipe: false,
            autocd: false,
            cdspell: false,
            checkwinsize: false,
            inherit_errexit: false,
            config_source: ConfigSource::Bashrc,
        }
    }
}

/// Hook lists for shell events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShellHooks {
    pub precmd: Vec<String>,   // run before each prompt
    pub preexec: Vec<String>,  // run before each command
    pub chpwd: Vec<String>,    // run after directory change
}

/// Completion specification for a command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionSpec {
    pub command: String,
    pub word_list: Option<Vec<String>>,
    pub function: Option<String>,
    pub directory: bool,
    pub file: bool,
    pub glob_pattern: Option<String>,
    pub filter_pattern: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

pub struct ShellState {
    pub env_vars: HashMap<String, String>,
    pub local_vars_stack: Vec<HashMap<String, String>>,
    pub aliases: HashMap<String, String>,
    pub functions: HashMap<String, CompoundCommand>,
    pub last_exit_code: i32,
    pub last_bg_pid: Option<u32>,
    pub interactive: bool,
    pub home_dir: PathBuf,
    pub hostname: String,
    path_cache: Option<HashSet<String>>,
    path_hash: u64, // Hash of current PATH for quick comparison
    pub positional_params: Vec<String>,
    pub positional_stack: Vec<Vec<String>>,
    pub dir_stack: Vec<PathBuf>,
    pub shell_opts: ShellOpts,
    pub traps: HashMap<String, String>,
    pub pipestatus: Vec<i32>,
    pub jobs: JobTable,
    // Arrays (Phase 1)
    pub arrays: HashMap<String, Vec<String>>,
    pub assoc_arrays: HashMap<String, HashMap<String, String>>,
    // Hooks (Phase 4)
    pub hooks: ShellHooks,
    // Completion specs (Phase 7)
    pub completion_specs: HashMap<String, CompletionSpec>,
    // Notification threshold (Phase 8)
    pub notification_threshold: std::time::Duration,
    // Last command duration for rprompt
    pub last_command_duration: Option<std::time::Duration>,
    // Editing mode (vi or emacs)
    pub editing_mode: EditingMode,
    // Prompt style and terminal width
    pub prompt_style: PromptStyle,
    pub terminal_width: usize,
    // Loop control flow (break/continue)
    pub loop_break: bool,
    pub loop_continue: bool,
    // Function return control flow
    pub return_requested: bool,
    pub return_value: i32,
}

impl ShellState {
    pub fn new(interactive: bool) -> Self {
        let mut env_vars = HashMap::new();
        for (k, v) in env::vars() {
            env_vars.insert(k, v);
        }

        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let hostname = std::fs::read_to_string("/etc/hostname")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| String::from("localhost"));

        let path_hash = Self::hash_path(env_vars.get("PATH").map(|s| s.as_str()).unwrap_or(""));

        ShellState {
            env_vars,
            local_vars_stack: Vec::new(),
            aliases: HashMap::new(),
            functions: HashMap::new(),
            last_exit_code: 0,
            last_bg_pid: None,
            interactive,
            home_dir,
            hostname,
            path_cache: None,
            path_hash,
            positional_params: Vec::new(),
            positional_stack: Vec::new(),
            dir_stack: Vec::new(),
            shell_opts: ShellOpts::default(),
            traps: HashMap::new(),
            pipestatus: Vec::new(),
            jobs: JobTable::new(),
            arrays: HashMap::new(),
            assoc_arrays: HashMap::new(),
            hooks: ShellHooks::default(),
            completion_specs: HashMap::new(),
            notification_threshold: std::time::Duration::from_secs(10),
            last_command_duration: None,
            editing_mode: EditingMode::Emacs,
            prompt_style: PromptStyle::Auto,
            terminal_width: Self::detect_terminal_width(),
            loop_break: false,
            loop_continue: false,
            return_requested: false,
            return_value: 0,
        }
    }

    fn hash_path(path: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        hasher.finish()
    }

    pub fn get_var(&self, name: &str) -> Option<&str> {
        match name {
            "?" => return None, // handled by expand
            _ => {}
        }
        // Check local_vars_stack from top to bottom
        for scope in self.local_vars_stack.iter().rev() {
            if let Some(val) = scope.get(name) {
                return Some(val.as_str());
            }
        }
        // Fall back to env_vars
        self.env_vars.get(name)
            .map(|s| s.as_str())
    }

    fn detect_terminal_width() -> usize {
        // Try COLUMNS environment variable first
        if let Ok(cols_str) = env::var("COLUMNS") {
            if let Ok(cols) = cols_str.parse::<usize>() {
                if cols > 0 {
                    return cols;
                }
            }
        }

        // Try stty size
        if let Ok(output) = std::process::Command::new("stty")
            .arg("size")
            .output() {
            if let Ok(output_str) = String::from_utf8(output.stdout) {
                let parts: Vec<&str> = output_str.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(cols) = parts[1].parse::<usize>() {
                        if cols > 0 {
                            return cols;
                        }
                    }
                }
            }
        }

        // Default fallback
        80
    }


    pub fn set_var(&mut self, name: &str, value: &str) {
        if self.env_vars.contains_key(name) {
            self.env_vars.insert(name.to_string(), value.to_string());
            env::set_var(name, value);
            if name == "PATH" {
                self.invalidate_path_cache();
            }
        } else if let Some(scope) = self.local_vars_stack.last_mut() {
            // In function scope: set in current local scope
            scope.insert(name.to_string(), value.to_string());
        } else {
            // At global scope: set in env_vars
            self.env_vars.insert(name.to_string(), value.to_string());
            env::set_var(name, value);
            if name == "PATH" {
                self.invalidate_path_cache();
            }
        }
    }

    pub fn export_var(&mut self, name: &str, value: &str) {
        self.env_vars.insert(name.to_string(), value.to_string());
        env::set_var(name, value);
        // Remove from all local scopes
        for scope in &mut self.local_vars_stack {
            scope.remove(name);
        }
        if name == "PATH" {
            self.invalidate_path_cache();
        }
    }

    pub fn unset_var(&mut self, name: &str) {
        self.env_vars.remove(name);
        // Remove from all local scopes
        for scope in &mut self.local_vars_stack {
            scope.remove(name);
        }
        self.arrays.remove(name);
        self.assoc_arrays.remove(name);
        env::remove_var(name);
        if name == "PATH" {
            self.invalidate_path_cache();
        }
    }

    fn invalidate_path_cache(&mut self) {
        self.path_cache = None;
        // Update the hash for the next check
        self.path_hash = Self::hash_path(
            self.env_vars.get("PATH").map(|s| s.as_str()).unwrap_or("")
        );
    }

    pub fn path_cache(&mut self) -> &HashSet<String> {
        // Check if PATH has changed (using hash for quick comparison)
        let current_path_hash = Self::hash_path(
            self.env_vars.get("PATH").map(|s| s.as_str()).unwrap_or("")
        );

        if self.path_hash != current_path_hash {
            self.path_cache = None;
            self.path_hash = current_path_hash;
        }

        // Rebuild cache only if it's None
        if self.path_cache.is_none() {
            let mut cache = HashSet::new();
            if let Some(path) = self.env_vars.get("PATH") {
                for dir in path.split(':') {
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for entry in entries.flatten() {
                            if let Ok(ft) = entry.file_type() {
                                if ft.is_file() || ft.is_symlink() {
                                    if let Some(name) = entry.file_name().to_str() {
                                        cache.insert(name.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            self.path_cache = Some(cache);
        }
        self.path_cache.as_ref().unwrap()
    }

    pub fn command_in_path(&mut self, name: &str) -> bool {
        self.path_cache().contains(name)
    }

    pub fn push_positional_params(&mut self, args: Vec<String>) {
        self.positional_stack.push(std::mem::replace(&mut self.positional_params, args));
    }

    pub fn pop_positional_params(&mut self) {
        if let Some(old) = self.positional_stack.pop() {
            self.positional_params = old;
        }
    }

    // Array helpers
    pub fn get_array_element(&self, name: &str, index: &str) -> Option<String> {
        if let Some(arr) = self.arrays.get(name) {
            let idx: usize = index.parse().ok()?;
            arr.get(idx).cloned()
        } else if let Some(map) = self.assoc_arrays.get(name) {
            map.get(index).cloned()
        } else {
            None
        }
    }

    pub fn set_array_element(&mut self, name: &str, index: &str, value: &str) {
        if self.assoc_arrays.contains_key(name) {
            self.assoc_arrays.get_mut(name).unwrap()
                .insert(index.to_string(), value.to_string());
        } else {
            let arr = self.arrays.entry(name.to_string()).or_default();
            if let Ok(idx) = index.parse::<usize>() {
                if idx >= arr.len() {
                    arr.resize(idx + 1, String::new());
                }
                arr[idx] = value.to_string();
            }
        }
    }

    pub fn array_values(&self, name: &str) -> Vec<String> {
        if let Some(arr) = self.arrays.get(name) {
            arr.clone()
        } else if let Some(map) = self.assoc_arrays.get(name) {
            map.values().cloned().collect()
        } else {
            Vec::new()
        }
    }

    pub fn array_keys(&self, name: &str) -> Vec<String> {
        if let Some(arr) = self.arrays.get(name) {
            (0..arr.len()).map(|i| i.to_string()).collect()
        } else if let Some(map) = self.assoc_arrays.get(name) {
            map.keys().cloned().collect()
        } else {
            Vec::new()
        }
    }

    pub fn array_length(&self, name: &str) -> usize {
        if let Some(arr) = self.arrays.get(name) {
            arr.len()
        } else if let Some(map) = self.assoc_arrays.get(name) {
            map.len()
        } else {
            0
        }
    }

    pub fn is_array(&self, name: &str) -> bool {
        self.arrays.contains_key(name) || self.assoc_arrays.contains_key(name)
    }

    /// Push a new local variable scope (for function entry)
    pub fn push_local_scope(&mut self) {
        self.local_vars_stack.push(HashMap::new());
    }

    /// Pop the current local variable scope (for function exit)
    pub fn pop_local_scope(&mut self) {
        self.local_vars_stack.pop();
    }
}
