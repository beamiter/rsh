/// Tab completion engine: context-aware completion for commands, paths, variables.

use crate::environment::ShellState;
use std::fs;

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub display: String,
    pub is_dir: bool,
}

/// Perform completion for the current buffer at the cursor position.
/// Returns the start position of the word being completed and the list of completions.
pub fn complete(buffer: &str, cursor: usize, state: &mut ShellState) -> (usize, Vec<Completion>) {
    let buf = &buffer[..cursor];
    let (word, word_start) = extract_word_at(buf);
    let is_cmd_pos = is_command_position(buf, word_start);

    let cmd = first_command(buf);
    let completions = if word.starts_with('$') {
        complete_variable(&word[1..], state)
    } else if is_cmd_pos {
        complete_command(&word, state)
    } else if let Some(subs) = subcommand_completions(&cmd, &word, buf, word_start) {
        subs
    } else if cmd == "cd" || cmd == "mkdir" || cmd == "rmdir" {
        complete_path(&word, state).into_iter().filter(|c| c.is_dir).collect()
    } else {
        complete_path(&word, state)
    };

    (word_start, completions)
}

/// Extract the first command word from the buffer (before cursor).
fn first_command(buf: &str) -> String {
    let trimmed = buf.trim_start();
    // Find after the last pipe/semicolon/&& /|| to get the current simple command
    let cmd_start = trimmed.rfind(|c: char| c == '|' || c == ';')
        .map(|i| i + 1)
        .unwrap_or(0);
    let segment = trimmed[cmd_start..].trim_start();
    segment.split_whitespace().next().unwrap_or("").to_string()
}

/// Return subcommand completions for known commands, or None to fall back to path completion.
fn subcommand_completions(cmd: &str, prefix: &str, buf: &str, word_start: usize) -> Option<Vec<Completion>> {
    // Only complete subcommands in the second word position
    let before = buf[..word_start].trim_end();
    let word_count = before.split_whitespace().count();
    // For "git ch", before is "git", word_count is 1 → second position
    // For "git checkout ma", before is "git checkout", word_count is 2 → third position (path completion)
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
        _ => return None,
    };

    let completions = subs.iter()
        .filter(|s| s.starts_with(prefix))
        .map(|s| Completion {
            text: s.to_string(),
            display: s.to_string(),
            is_dir: false,
        })
        .collect::<Vec<_>>();

    Some(completions)
}

fn extract_word_at(buf: &str) -> (String, usize) {
    // Find the start of the current word (going backwards)
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

    // Builtins
    for cmd in crate::builtins::BUILTIN_NAMES {
        if cmd.starts_with(prefix) {
            completions.push(Completion {
                text: cmd.to_string(),
                display: cmd.to_string(),
                is_dir: false,
            });
        }
    }

    // Aliases
    for name in state.aliases.keys() {
        if name.starts_with(prefix) {
            completions.push(Completion {
                text: name.clone(),
                display: format!("{} (alias)", name),
                is_dir: false,
            });
        }
    }

    // Functions
    for name in state.functions.keys() {
        if name.starts_with(prefix) {
            completions.push(Completion {
                text: name.clone(),
                display: format!("{} (function)", name),
                is_dir: false,
            });
        }
    }

    // PATH executables
    for cmd in state.path_cache().iter() {
        if cmd.starts_with(prefix) {
            completions.push(Completion {
                text: cmd.clone(),
                display: cmd.clone(),
                is_dir: false,
            });
        }
    }

    // If prefix contains /, also try path completion
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
            Some(pos) => (&expanded[..=pos], &expanded[pos + 1..])  ,
            None => (".", expanded.as_str()),
        }
    };

    let mut completions = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(file_prefix) { continue; }
            // Skip hidden files unless prefix starts with .
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
                is_dir: false,
            });
        }
    }
    for name in state.local_vars.keys() {
        if name.starts_with(prefix) {
            completions.push(Completion {
                text: format!("${}", name),
                display: name.clone(),
                is_dir: false,
            });
        }
    }

    completions.sort_by(|a, b| a.text.cmp(&b.text));
    completions
}

/// Find the longest common prefix among completions.
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
