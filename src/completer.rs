/// Tab completion engine: context-aware completion for commands, paths, variables,
/// with configurable completion specs (Phase 7).

use crate::environment::ShellState;
use std::fs;

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub display: String,
    pub description: Option<String>,
    pub is_dir: bool,
}

pub fn complete(buffer: &str, cursor: usize, state: &mut ShellState) -> (usize, Vec<Completion>) {
    let buf = &buffer[..cursor];
    let (word, word_start) = extract_word_at(buf);
    let is_cmd_pos = is_command_position(buf, word_start);

    let cmd = first_command(buf);

    // Check user-defined completion specs first
    if !is_cmd_pos {
        if let Some(spec) = state.completion_specs.get(&cmd).cloned() {
            let completions = apply_completion_spec(&spec, &word, state);
            if !completions.is_empty() {
                return (word_start, completions);
            }
        }
    }

    let completions = if word.starts_with('$') {
        complete_variable(&word[1..], state)
    } else if is_cmd_pos {
        complete_command(&word, state)
    } else if let Some(subs) = subcommand_completions(&cmd, &word, buf, word_start) {
        subs
    } else if cmd == "cd" || cmd == "mkdir" || cmd == "rmdir" || cmd == "z" {
        complete_path(&word, state).into_iter().filter(|c| c.is_dir).collect()
    } else {
        complete_path(&word, state)
    };

    (word_start, completions)
}

fn apply_completion_spec(spec: &crate::environment::CompletionSpec, prefix: &str, state: &mut ShellState) -> Vec<Completion> {
    let mut completions = Vec::new();

    // -W word list
    if let Some(ref words) = spec.word_list {
        for w in words {
            if w.starts_with(prefix) {
                completions.push(Completion {
                    text: w.clone(),
                    display: w.clone(),
                    description: None,
                    is_dir: false,
                });
            }
        }
    }

    // -d directory
    if spec.directory {
        completions.extend(complete_path(prefix, state).into_iter().filter(|c| c.is_dir));
    }

    // -f file
    if spec.file {
        completions.extend(complete_path(prefix, state));
    }

    // -X filter pattern
    if let Some(ref pattern) = spec.filter_pattern {
        completions.retain(|c| !crate::glob_match::glob_match(pattern, &c.text));
    }

    // -P prefix, -S suffix
    if let Some(ref pfx) = spec.prefix {
        for c in &mut completions { c.text = format!("{}{}", pfx, c.text); }
    }
    if let Some(ref sfx) = spec.suffix {
        for c in &mut completions { c.text = format!("{}{}", c.text, sfx); }
    }

    completions
}

fn first_command(buf: &str) -> String {
    let trimmed = buf.trim_start();
    let cmd_start = trimmed.rfind(|c: char| c == '|' || c == ';')
        .map(|i| i + 1)
        .unwrap_or(0);
    let segment = trimmed[cmd_start..].trim_start();
    segment.split_whitespace().next().unwrap_or("").to_string()
}

fn subcommand_completions(cmd: &str, prefix: &str, buf: &str, word_start: usize) -> Option<Vec<Completion>> {
    let before = buf[..word_start].trim_end();
    let word_count = before.split_whitespace().count();
    if word_count != 1 {
        return None;
    }

    let subs: &[&str] = match cmd {
        "git" => &[
            "add", "bisect", "blame", "branch", "checkout", "cherry-pick",
            "clone", "commit", "config", "diff", "fetch", "grep", "init",
            "log", "merge", "mv", "pull", "push", "rebase", "remote",
            "reset", "restore", "revert", "rm", "show", "stash", "status",
            "switch", "tag", "worktree",
        ],
        "cargo" => &[
            "add", "bench", "build", "check", "clean", "clippy", "doc",
            "fetch", "fix", "fmt", "init", "install", "new", "publish",
            "remove", "run", "search", "test", "tree", "uninstall", "update",
        ],
        "docker" => &[
            "build", "compose", "container", "cp", "create", "exec",
            "image", "images", "kill", "logs", "network", "ps", "pull",
            "push", "restart", "rm", "rmi", "run", "start", "stop",
            "tag", "volume",
        ],
        "systemctl" => &[
            "daemon-reload", "disable", "edit", "enable", "is-active",
            "is-enabled", "list-units", "reload", "restart", "start",
            "status", "stop",
        ],
        "npm" => &[
            "audit", "build", "cache", "ci", "clean", "config", "create",
            "exec", "init", "install", "link", "list", "outdated", "pack",
            "publish", "rebuild", "remove", "run", "search", "start",
            "test", "uninstall", "update", "version",
        ],
        "hook" => &["add", "remove", "list"],
        _ => return None,
    };

    let completions = subs.iter()
        .filter(|s| s.starts_with(prefix))
        .map(|s| Completion {
            text: s.to_string(),
            display: s.to_string(),
            description: None,
            is_dir: false,
        })
        .collect::<Vec<_>>();

    Some(completions)
}

fn extract_word_at(buf: &str) -> (String, usize) {
    let bytes = buf.as_bytes();
    let mut start = buf.len();
    for i in (0..buf.len()).rev() {
        match bytes[i] {
            b' ' | b'\t' | b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => {
                start = i + 1;
                break;
            }
            _ => {
                if i == 0 { start = 0; }
            }
        }
    }
    let word = buf[start..].to_string();
    (word, start)
}

fn is_command_position(buf: &str, word_start: usize) -> bool {
    let before = buf[..word_start].trim_end();
    before.is_empty() ||
    before.ends_with('|') ||
    before.ends_with("&&") ||
    before.ends_with("||") ||
    before.ends_with(';') ||
    before.ends_with('(') ||
    before.ends_with('{')
}

fn complete_command(prefix: &str, state: &mut ShellState) -> Vec<Completion> {
    let mut completions = Vec::new();

    for cmd in crate::builtins::BUILTIN_NAMES {
        if cmd.starts_with(prefix) {
            completions.push(Completion {
                text: cmd.to_string(),
                display: cmd.to_string(),
                description: Some("builtin".to_string()),
                is_dir: false,
            });
        }
    }

    for name in state.aliases.keys() {
        if name.starts_with(prefix) {
            completions.push(Completion {
                text: name.clone(),
                display: name.clone(),
                description: Some("alias".to_string()),
                is_dir: false,
            });
        }
    }

    for name in state.functions.keys() {
        if name.starts_with(prefix) {
            completions.push(Completion {
                text: name.clone(),
                display: name.clone(),
                description: Some("function".to_string()),
                is_dir: false,
            });
        }
    }

    for cmd in state.path_cache().iter() {
        if cmd.starts_with(prefix) {
            completions.push(Completion {
                text: cmd.clone(),
                display: cmd.clone(),
                description: None,
                is_dir: false,
            });
        }
    }

    if prefix.contains('/') {
        completions.extend(complete_path(prefix, state));
    }

    completions.sort_by(|a, b| a.text.cmp(&b.text));
    completions.dedup_by(|a, b| a.text == b.text);
    completions
}

fn complete_path(prefix: &str, state: &ShellState) -> Vec<Completion> {
    let expanded = if prefix.starts_with('~') {
        let home = state.home_dir.to_string_lossy();
        if prefix == "~" {
            format!("{}/", home)
        } else {
            format!("{}{}", home, &prefix[1..])
        }
    } else {
        prefix.to_string()
    };

    let (dir, file_prefix) = if expanded.ends_with('/') {
        (expanded.as_str(), "")
    } else {
        match expanded.rfind('/') {
            Some(pos) => (&expanded[..=pos], &expanded[pos + 1..]),
            None => (".", expanded.as_str()),
        }
    };

    let mut completions = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(file_prefix) { continue; }
            if name.starts_with('.') && !file_prefix.starts_with('.') { continue; }

            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let full = if dir == "." {
                if is_dir { format!("{}/", name) } else { name.clone() }
            } else if prefix.starts_with('~') {
                let suffix = if expanded.ends_with('/') {
                    format!("{}{}", &prefix, name)
                } else {
                    match prefix.rfind('/') {
                        Some(pos) => format!("{}/{}", &prefix[..pos], name),
                        None => format!("~/{}", name),
                    }
                };
                if is_dir { format!("{}/", suffix) } else { suffix }
            } else {
                let path = if expanded.ends_with('/') {
                    format!("{}{}", prefix, name)
                } else {
                    match prefix.rfind('/') {
                        Some(pos) => format!("{}/{}", &prefix[..pos], name),
                        None => name.clone(),
                    }
                };
                if is_dir { format!("{}/", path) } else { path }
            };

            completions.push(Completion {
                text: full,
                display: if is_dir { format!("{}/", name) } else { name },
                description: None,
                is_dir,
            });
        }
    }

    completions.sort_by(|a, b| a.text.cmp(&b.text));
    completions
}

fn complete_variable(prefix: &str, state: &ShellState) -> Vec<Completion> {
    let mut completions = Vec::new();

    for name in state.env_vars.keys() {
        if name.starts_with(prefix) {
            completions.push(Completion {
                text: format!("${}", name),
                display: name.clone(),
                description: None,
                is_dir: false,
            });
        }
    }
    for name in state.local_vars.keys() {
        if name.starts_with(prefix) {
            completions.push(Completion {
                text: format!("${}", name),
                display: name.clone(),
                description: None,
                is_dir: false,
            });
        }
    }
    // Also complete array names
    for name in state.arrays.keys() {
        if name.starts_with(prefix) {
            completions.push(Completion {
                text: format!("${}", name),
                display: format!("{} (array)", name),
                description: Some("array".to_string()),
                is_dir: false,
            });
        }
    }
    for name in state.assoc_arrays.keys() {
        if name.starts_with(prefix) {
            completions.push(Completion {
                text: format!("${}", name),
                display: format!("{} (assoc)", name),
                description: Some("assoc array".to_string()),
                is_dir: false,
            });
        }
    }

    completions.sort_by(|a, b| a.text.cmp(&b.text));
    completions
}

pub fn common_prefix(completions: &[Completion]) -> String {
    if completions.is_empty() { return String::new(); }
    let first = &completions[0].text;
    let mut len = first.len();
    for c in &completions[1..] {
        len = len.min(c.text.len());
        for (i, (a, b)) in first.chars().zip(c.text.chars()).enumerate() {
            if a != b && i < len {
                len = i;
                break;
            }
        }
    }
    first[..len].to_string()
}
