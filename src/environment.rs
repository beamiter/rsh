use std::collections::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;

use crate::job::JobTable;
use crate::parser::ast::CompoundCommand;

#[derive(Debug, Clone, Default)]
pub struct ShellOpts {
    pub errexit: bool,  // set -e
    pub xtrace: bool,   // set -x
    pub pipefail: bool,  // set -o pipefail
}

/// Hook lists for shell events.
#[derive(Debug, Clone, Default)]
pub struct ShellHooks {
    pub precmd: Vec<String>,   // run before each prompt
    pub preexec: Vec<String>,  // run before each command
    pub chpwd: Vec<String>,    // run after directory change
}

/// Completion specification for a command.
#[derive(Debug, Clone)]
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
    pub local_vars: HashMap<String, String>,
    pub aliases: HashMap<String, String>,
    pub functions: HashMap<String, CompoundCommand>,
    pub last_exit_code: i32,
    pub last_bg_pid: Option<u32>,
    pub interactive: bool,
    pub home_dir: PathBuf,
    pub hostname: String,
    path_cache: Option<HashSet<String>>,
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

        ShellState {
            env_vars,
            local_vars: HashMap::new(),
            aliases: HashMap::new(),
            functions: HashMap::new(),
            last_exit_code: 0,
            last_bg_pid: None,
            interactive,
            home_dir,
            hostname,
            path_cache: None,
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
        }
    }

    pub fn get_var(&self, name: &str) -> Option<&str> {
        match name {
            "?" => return None, // handled by expand
            _ => {}
        }
        self.local_vars.get(name)
            .or_else(|| self.env_vars.get(name))
            .map(|s| s.as_str())
    }

    pub fn set_var(&mut self, name: &str, value: &str) {
        if self.env_vars.contains_key(name) {
            self.env_vars.insert(name.to_string(), value.to_string());
            env::set_var(name, value);
            if name == "PATH" {
                self.invalidate_path_cache();
            }
        } else {
            self.local_vars.insert(name.to_string(), value.to_string());
        }
    }

    pub fn export_var(&mut self, name: &str, value: &str) {
        self.env_vars.insert(name.to_string(), value.to_string());
        env::set_var(name, value);
        self.local_vars.remove(name);
        if name == "PATH" {
            self.invalidate_path_cache();
        }
    }

    pub fn unset_var(&mut self, name: &str) {
        self.env_vars.remove(name);
        self.local_vars.remove(name);
        self.arrays.remove(name);
        self.assoc_arrays.remove(name);
        env::remove_var(name);
        if name == "PATH" {
            self.invalidate_path_cache();
        }
    }

    fn invalidate_path_cache(&mut self) {
        self.path_cache = None;
    }

    pub fn path_cache(&mut self) -> &HashSet<String> {
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
}
