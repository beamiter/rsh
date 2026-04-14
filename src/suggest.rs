/// Auto-suggestion engine: fish-style ghost text from history + z-jump,
/// with context-aware git suggestions and sequential command recommendations.

use std::collections::HashMap;
use crate::history::History;

/// Context passed to the suggestion engine (zero-allocation, borrows from ShellState).
pub struct SuggestionContext<'a> {
    pub git_branch: Option<&'a str>,
    pub last_command: Option<&'a str>,
    pub last_exit_code: i32,
}

/// Static command chain patterns: (prefix of last command) -> suggested next command.
/// Order matters: first match wins.
const COMMAND_CHAINS: &[(&str, &str)] = &[
    ("git commit", "git push"),
    ("git add", "git commit"),
    ("git stash pop", "git diff"),
    ("git stash", "git stash pop"),
    ("git pull", "git diff"),
    ("git clone", "cd "),
    ("cargo build", "cargo run"),
    ("cargo test", "cargo build"),
    ("cargo fmt", "cargo clippy"),
    ("docker build", "docker run"),
    ("mkdir", "cd "),
    ("npm install", "npm run"),
    ("make", "make install"),
];

/// Given the current buffer, find a suggestion from history, git context, or z-jump.
/// Returns the suffix to display as ghost text (the part after the buffer).
pub fn suggest(buffer: &str, history: &History, ctx: &SuggestionContext) -> Option<String> {
    // 0. Empty buffer: proactive sequential command suggestion
    if buffer.is_empty() {
        return suggest_next_command(ctx, history);
    }

    // 1. Exact prefix match from history (best, current behavior)
    if let Some(entry) = history.search_prefix(buffer) {
        return Some(entry[buffer.len()..].to_string());
    }

    // 2. Git-aware suggestions (context-sensitive)
    if buffer.starts_with("git ") {
        if let Some(s) = suggest_git_command(buffer, ctx) {
            return Some(s);
        }
    }

    // 3. For "cd " commands, suggest from z-jump database
    if buffer.starts_with("cd ") {
        let query = buffer[3..].trim();
        if !query.is_empty() {
            if let Ok(db) = crate::zjump::get_z_db().lock() {
                if let Some(path) = db.query(&[query]) {
                    // Return the full path as suggestion, replacing the partial arg
                    let current_arg = buffer[3..].to_string();
                    if path.len() > current_arg.len() && path.contains(query) {
                        return Some(path[current_arg.len()..].to_string());
                    }
                    // Or show full path after "cd "
                    let suggestion = format!("{}", &path);
                    if suggestion.starts_with(query) {
                        return Some(suggestion[query.len()..].to_string());
                    }
                    // Fallback: suggest full replacement
                    return Some(format!(" # -> {}", path));
                }
            }
        }
    }

    None
}

/// Git-aware suggestions: auto-complete `git push/pull` with `origin <branch>`.
fn suggest_git_command(buffer: &str, ctx: &SuggestionContext) -> Option<String> {
    let branch = ctx.git_branch?;

    for cmd in &["git push", "git pull"] {
        // "git push" or "git pull" (no trailing space)
        if buffer == *cmd {
            return Some(format!(" origin {}", branch));
        }
        // "git push " (with trailing space, no remote yet)
        let with_space = format!("{} ", cmd);
        if buffer == with_space {
            return Some(format!("origin {}", branch));
        }
        // "git push origin" (no trailing space after origin)
        let with_origin = format!("{} origin", cmd);
        if buffer == with_origin {
            return Some(format!(" {}", branch));
        }
        // "git push origin " (with trailing space, ready for branch)
        let origin_space = format!("{} origin ", cmd);
        if buffer == origin_space {
            return Some(branch.to_string());
        }
        // "git push origin ma" -> suggest "ster" if branch is "master"
        if buffer.starts_with(&origin_space) {
            let partial = &buffer[origin_space.len()..];
            if !partial.is_empty() && branch.starts_with(partial) && branch.len() > partial.len() {
                return Some(branch[partial.len()..].to_string());
            }
        }
    }

    // git checkout / git switch: suggest branch name
    for cmd in &["git checkout", "git switch"] {
        let prefix = format!("{} ", cmd);
        if buffer == *cmd {
            // Don't auto-suggest branch here, user might want a file
            continue;
        }
        if buffer.starts_with(&prefix) {
            let partial = &buffer[prefix.len()..];
            if !partial.is_empty() && !partial.starts_with('-') {
                // Don't override if already has a complete branch
                if branch.starts_with(partial) && branch.len() > partial.len() {
                    return Some(branch[partial.len()..].to_string());
                }
            }
        }
    }

    None
}

/// Proactive suggestion when the buffer is empty: recommend the next command
/// based on static chain rules and learned history patterns.
fn suggest_next_command(ctx: &SuggestionContext, history: &History) -> Option<String> {
    let last_cmd = ctx.last_command?;

    // Only suggest after successful commands
    if ctx.last_exit_code != 0 {
        return None;
    }

    // 1. Check static chain patterns first
    for (prefix, suggestion) in COMMAND_CHAINS {
        if last_cmd.starts_with(prefix) {
            // Enrich git push with branch info
            if *suggestion == "git push" {
                if let Some(branch) = ctx.git_branch {
                    return Some(format!("git push origin {}", branch));
                }
            }
            return Some(suggestion.to_string());
        }
    }

    // 2. Fall back to history-based chain learning
    suggest_from_history_chains(last_cmd, history)
}

/// Learn command chains from history: find the most common successor command.
fn suggest_from_history_chains(last_cmd: &str, history: &History) -> Option<String> {
    let entries = history.entries();
    if entries.len() < 2 {
        return None;
    }

    let last_base = command_base(last_cmd);

    // Count successor commands
    let mut successors: HashMap<&str, u32> = HashMap::new();
    for window in entries.windows(2) {
        if command_base(&window[0]) == last_base {
            *successors.entry(&window[1]).or_insert(0) += 1;
        }
    }

    // Require at least 3 occurrences to avoid noise
    successors.into_iter()
        .filter(|(_, count)| *count >= 3)
        .max_by_key(|(_, count)| *count)
        .map(|(cmd, _)| cmd.to_string())
}

/// Extract the "base" of a command for chain matching.
/// For compound commands like "git commit", uses the first two words.
/// For simple commands like "ls", uses just the first word.
fn command_base(cmd: &str) -> &str {
    let trimmed = cmd.trim();
    let mut words = trimmed.split_whitespace();
    let first = match words.next() {
        Some(w) => w,
        None => return trimmed,
    };

    // For known multi-word command families, include the subcommand
    match first {
        "git" | "cargo" | "docker" | "kubectl" | "npm" | "pip" | "pip3" | "go" | "make" => {
            if let Some(second) = words.next() {
                // Return slice covering "first second"
                let start = first.as_ptr() as usize - trimmed.as_ptr() as usize;
                let end = second.as_ptr() as usize - trimmed.as_ptr() as usize + second.len();
                &trimmed[start..end]
            } else {
                first
            }
        }
        _ => first,
    }
}
