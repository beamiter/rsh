use std::collections::HashMap;
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

pub struct ShellState {
    pub env_vars: HashMap<String, String>,
    pub local_vars: HashMap<String, String>,
    pub aliases: HashMap<String, String>,
    pub functions: HashMap<String, CompoundCommand>,
    pub last_exit_code: i32,
    pub last_bg_pid: Option<u32>,
    pub interactive: bool,
    pub home_dir: PathBuf,
    pub path_cache: Vec<String>,
    pub positional_params: Vec<String>,
    pub positional_stack: Vec<Vec<String>>,
    pub dir_stack: Vec<PathBuf>,
    pub shell_opts: ShellOpts,
    pub traps: HashMap<String, String>,
    pub pipestatus: Vec<i32>,
    pub jobs: JobTable,
}

impl ShellState {
    pub fn new(interactive: bool) -> Self {
        let mut env_vars = HashMap::new();
        for (k, v) in env::vars() {
            env_vars.insert(k, v);
        }

        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));

        let mut state = ShellState {
            env_vars,
            local_vars: HashMap::new(),
            aliases: HashMap::new(),
            functions: HashMap::new(),
            last_exit_code: 0,
            last_bg_pid: None,
            interactive,
            home_dir,
            path_cache: Vec::new(),
            positional_params: Vec::new(),
            positional_stack: Vec::new(),
            dir_stack: Vec::new(),
            shell_opts: ShellOpts::default(),
            traps: HashMap::new(),
            pipestatus: Vec::new(),
            jobs: JobTable::new(),
        };
        state.rebuild_path_cache();
        state
    }

    pub fn get_var(&self, name: &str) -> Option<&str> {
        // Special variables
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
                self.rebuild_path_cache();
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
            self.rebuild_path_cache();
        }
    }

    pub fn unset_var(&mut self, name: &str) {
        self.env_vars.remove(name);
        self.local_vars.remove(name);
        env::remove_var(name);
        if name == "PATH" {
            self.rebuild_path_cache();
        }
    }

    pub fn rebuild_path_cache(&mut self) {
        self.path_cache.clear();
        if let Some(path) = self.env_vars.get("PATH") {
            for dir in path.split(':') {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        if let Ok(ft) = entry.file_type() {
                            if ft.is_file() || ft.is_symlink() {
                                if let Some(name) = entry.file_name().to_str() {
                                    self.path_cache.push(name.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        self.path_cache.sort();
        self.path_cache.dedup();
    }

    pub fn command_in_path(&self, name: &str) -> bool {
        self.path_cache.binary_search(&name.to_string()).is_ok()
    }

    pub fn push_positional_params(&mut self, args: Vec<String>) {
        self.positional_stack.push(std::mem::replace(&mut self.positional_params, args));
    }

    pub fn pop_positional_params(&mut self) {
        if let Some(old) = self.positional_stack.pop() {
            self.positional_params = old;
        }
    }
}
