/// Tab completion engine: context-aware completion for commands, paths, variables,
/// with configurable completion specs (Phase 7).

use crate::environment::ShellState;
use std::fs;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub display: String,
    pub description: Option<String>,
    pub is_dir: bool,
}

/// Completion cache entry with frequency tracking
#[derive(Debug, Clone)]
struct CacheEntry {
    completions: Vec<Completion>,
    hit_count: u32,
}

/// LRU completion cache
#[derive(Debug)]
struct CompletionCache {
    cache: HashMap<String, CacheEntry>,
    max_size: usize,
}

impl CompletionCache {
    fn new(max_size: usize) -> Self {
        CompletionCache {
            cache: HashMap::new(),
            max_size,
        }
    }

    fn get(&mut self, key: &str) -> Option<Vec<Completion>> {
        if let Some(entry) = self.cache.get_mut(key) {
            entry.hit_count += 1;
            return Some(entry.completions.clone());
        }
        None
    }

    fn insert(&mut self, key: String, completions: Vec<Completion>) {
        if self.cache.len() >= self.max_size && !self.cache.contains_key(&key) {
            // Remove the least frequently used entry
            if let Some(lfu_key) = self.cache
                .iter()
                .min_by_key(|(_, entry)| entry.hit_count)
                .map(|(k, _)| k.clone())
            {
                self.cache.remove(&lfu_key);
            }
        }

        self.cache.insert(
            key,
            CacheEntry {
                completions,
                hit_count: 0,
            },
        );
    }

    fn clear(&mut self) {
        self.cache.clear();
    }
}

// Thread-local cache for completion results
thread_local! {
    static COMPLETION_CACHE: std::cell::RefCell<CompletionCache> =
        std::cell::RefCell::new(CompletionCache::new(256));
}

pub fn complete(buffer: &str, cursor: usize, state: &mut ShellState) -> (usize, Vec<Completion>) {
    let buf = &buffer[..cursor];
    let (word, word_start) = extract_word_at(buf);
    let is_cmd_pos = is_command_position(buf, word_start);

    let cmd = first_command(buf);

    // Create cache key based on context
    let cache_key = if is_cmd_pos {
        format!("cmd:{}", word)
    } else if word.starts_with('$') {
        format!("var:{}", &word[1..])
    } else {
        format!("path:{}", word)
    };

    // Try to get from cache
    let cached = COMPLETION_CACHE.with(|cache| {
        cache.borrow_mut().get(&cache_key)
    });

    if let Some(completions) = cached {
        return (word_start, completions);
    }

    // Check user-defined completion specs first
    if !is_cmd_pos {
        if let Some(spec) = state.completion_specs.get(&cmd).cloned() {
            let completions = apply_completion_spec(&spec, &word, state);
            if !completions.is_empty() {
                COMPLETION_CACHE.with(|cache| {
                    cache.borrow_mut().insert(cache_key, completions.clone());
                });
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

    // Store in cache
    COMPLETION_CACHE.with(|cache| {
        cache.borrow_mut().insert(cache_key, completions.clone());
    });

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

    // -F function
    if let Some(ref func_name) = spec.function {
        if let Some(func_body) = state.functions.get(func_name).cloned() {
            // Set completion variables - push a local scope for these variables
            state.push_local_scope();
            let line = prefix; // simplified
            let words: Vec<String> = line.split_whitespace().map(|s| s.to_string()).collect();
            state.arrays.insert("COMP_WORDS".to_string(), words.clone());
            if let Some(scope) = state.local_vars_stack.last_mut() {
                scope.insert("COMP_CWORD".to_string(), (words.len().saturating_sub(1)).to_string());
                scope.insert("COMP_LINE".to_string(), line.to_string());
                scope.insert("COMP_POINT".to_string(), line.len().to_string());
            }
            state.arrays.insert("COMPREPLY".to_string(), Vec::new());

            // Execute the function
            crate::executor::execute_compound(&func_body, state);

            // Read COMPREPLY
            if let Some(replies) = state.arrays.get("COMPREPLY") {
                for reply in replies {
                    if reply.starts_with(prefix) {
                        completions.push(Completion {
                            text: reply.clone(),
                            display: reply.clone(),
                            description: None,
                            is_dir: false,
                        });
                    }
                }
            }

            // Clean up - pop the local scope
            state.pop_local_scope();
            state.arrays.remove("COMP_WORDS");
            state.arrays.remove("COMPREPLY");
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
    let words: Vec<&str> = before.split_whitespace().collect();
    let word_count = words.len();

    // First-level subcommands
    if word_count == 1 {
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
            "bookmark" => &["add", "go", "ls", "rm"],
            "kubectl" => &[
                "apply", "attach", "auth", "config", "create", "delete",
                "describe", "diff", "edit", "exec", "expose", "get", "label",
                "logs", "patch", "port-forward", "proxy", "rollout", "run",
                "scale", "set", "top", "version",
            ],
            "pip" | "pip3" => &[
                "install", "uninstall", "download", "freeze", "list", "show",
                "search", "wheel", "hash", "check", "config", "cache",
            ],
            "go" => &[
                "build", "clean", "doc", "env", "fix", "fmt", "generate",
                "get", "install", "list", "mod", "run", "test", "tool",
                "version", "vet", "work",
            ],
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

        return Some(completions);
    }

    // Second-level: git branch/tag completion for specific subcommands
    if cmd == "git" && word_count >= 2 {
        let subcmd = words.get(1).copied().unwrap_or("");
        match subcmd {
            "checkout" | "switch" | "merge" | "rebase" | "branch" | "diff" | "log" => {
                return Some(complete_git_refs(prefix));
            }
            _ => {}
        }
    }

    // Second-level: docker compose subcommands
    if cmd == "docker" && word_count == 2 {
        let subcmd = words.get(1).copied().unwrap_or("");
        if subcmd == "compose" {
            let subs = &["build", "config", "create", "down", "events", "exec",
                "images", "kill", "logs", "ls", "pause", "port", "ps", "pull",
                "push", "restart", "rm", "run", "start", "stop", "top", "unpause", "up"];
            let completions = subs.iter()
                .filter(|s| s.starts_with(prefix))
                .map(|s| Completion {
                    text: s.to_string(),
                    display: s.to_string(),
                    description: None,
                    is_dir: false,
                })
                .collect::<Vec<_>>();
            return Some(completions);
        }
    }

    // Second-level: bookmark name completion for go/rm
    if cmd == "bookmark" && word_count == 2 {
        let subcmd = words.get(1).copied().unwrap_or("");
        if subcmd == "go" || subcmd == "rm" {
            if let Ok(db) = crate::bookmarks::get_bookmark_db().lock() {
                let completions = db.names().into_iter()
                    .filter(|n| n.starts_with(prefix))
                    .map(|n| Completion {
                        text: n.clone(),
                        display: n,
                        description: Some("bookmark".to_string()),
                        is_dir: false,
                    })
                    .collect::<Vec<_>>();
                return Some(completions);
            }
        }
    }

    // Option completions for common commands
    if prefix.starts_with('-') {
        let options: &[(&str, &str)] = match cmd {
            "ls" => &[
                ("-l", "long format"),
                ("-a", "include hidden"),
                ("-h", "human readable"),
                ("-r", "reverse order"),
                ("-t", "sort by time"),
                ("-S", "sort by size"),
                ("-R", "recursive"),
                ("-d", "list directories"),
            ],
            "grep" => &[
                ("-i", "case insensitive"),
                ("-v", "invert match"),
                ("-n", "show line numbers"),
                ("-r", "recursive"),
                ("-R", "recursive dereference"),
                ("-l", "list filenames"),
                ("-c", "count matches"),
                ("-o", "only matching parts"),
                ("-E", "extended regex"),
                ("-F", "fixed strings"),
            ],
            "find" => &[
                ("-type", "file type"),
                ("-name", "filename pattern"),
                ("-iname", "case insensitive name"),
                ("-path", "path pattern"),
                ("-regex", "regex pattern"),
                ("-size", "file size"),
                ("-mtime", "modification time"),
                ("-atime", "access time"),
                ("-user", "file owner"),
                ("-exec", "execute command"),
            ],
            "tar" => &[
                ("-c", "create archive"),
                ("-x", "extract archive"),
                ("-t", "list contents"),
                ("-v", "verbose"),
                ("-z", "gzip compression"),
                ("-j", "bzip2 compression"),
                ("-f", "archive file"),
                ("-C", "change directory"),
            ],
            "rm" => &[
                ("-r", "recursive"),
                ("-f", "force"),
                ("-i", "interactive"),
                ("-v", "verbose"),
            ],
            "cp" => &[
                ("-r", "recursive"),
                ("-i", "interactive"),
                ("-v", "verbose"),
                ("-a", "preserve all"),
                ("-p", "preserve properties"),
            ],
            "mkdir" => &[
                ("-p", "parents"),
                ("-m", "mode"),
                ("-v", "verbose"),
            ],
            "chmod" => &[
                ("-r", "recursive"),
                ("-v", "verbose"),
                ("-c", "changes only"),
                ("-R", "recursive"),
            ],
            _ => return None,
        };

        let completions = options.iter()
            .filter(|(opt, _)| opt.starts_with(prefix))
            .map(|(opt, desc)| Completion {
                text: opt.to_string(),
                display: opt.to_string(),
                description: Some(desc.to_string()),
                is_dir: false,
            })
            .collect::<Vec<_>>();

        if !completions.is_empty() {
            return Some(completions);
        }
    }

    None
}

fn complete_git_refs(prefix: &str) -> Vec<Completion> {
    let mut completions = Vec::new();

    // Get local branches
    if let Ok(output) = std::process::Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for branch in stdout.lines() {
                let branch = branch.trim();
                if !branch.is_empty() && branch.starts_with(prefix) {
                    completions.push(Completion {
                        text: branch.to_string(),
                        display: branch.to_string(),
                        description: Some("branch".to_string()),
                        is_dir: false,
                    });
                }
            }
        }
    }

    // Get tags
    if let Ok(output) = std::process::Command::new("git")
        .args(["tag", "-l"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for tag in stdout.lines() {
                let tag = tag.trim();
                if !tag.is_empty() && tag.starts_with(prefix) {
                    completions.push(Completion {
                        text: tag.to_string(),
                        display: tag.to_string(),
                        description: Some("tag".to_string()),
                        is_dir: false,
                    });
                }
            }
        }
    }

    // Get remote branches (without remote prefix for convenience)
    if let Ok(output) = std::process::Command::new("git")
        .args(["branch", "-r", "--format=%(refname:short)"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for branch in stdout.lines() {
                let branch = branch.trim();
                // Strip "origin/" prefix for convenience
                let short = branch.strip_prefix("origin/").unwrap_or(branch);
                if !short.is_empty() && short.starts_with(prefix) {
                    // Don't add if we already have it as a local branch
                    if !completions.iter().any(|c| c.text == short) {
                        completions.push(Completion {
                            text: short.to_string(),
                            display: short.to_string(),
                            description: Some("remote".to_string()),
                            is_dir: false,
                        });
                    }
                }
            }
        }
    }

    completions
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

    // Collect all builtin commands
    for cmd in crate::builtins::BUILTIN_NAMES {
        completions.push(Completion {
            text: cmd.to_string(),
            display: cmd.to_string(),
            description: Some("builtin".to_string()),
            is_dir: false,
        });
    }

    // Collect aliases
    for name in state.aliases.keys() {
        completions.push(Completion {
            text: name.clone(),
            display: name.clone(),
            description: Some("alias".to_string()),
            is_dir: false,
        });
    }

    // Collect functions
    for name in state.functions.keys() {
        completions.push(Completion {
            text: name.clone(),
            display: name.clone(),
            description: Some("function".to_string()),
            is_dir: false,
        });
    }

    // Collect commands in PATH
    for cmd in state.path_cache().iter() {
        completions.push(Completion {
            text: cmd.clone(),
            display: cmd.clone(),
            description: None,
            is_dir: false,
        });
    }

    // Add path completions if prefix contains /
    if prefix.contains('/') {
        completions.extend(complete_path(prefix, state));
    }

    // Remove duplicates
    completions.dedup_by(|a, b| a.text == b.text);

    // Apply fuzzy filtering and sorting
    let filtered = filter_completions(completions, prefix);

    // Limit to top 50 completions to avoid overwhelming the user
    filtered.into_iter().take(50).collect()
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

    // Add special shell variables first
    let special_vars = vec![
        ("?", "Last exit code"),
        ("!", "Last background PID"),
        ("*", "All positional parameters"),
        ("@", "All positional parameters (quoted)"),
        ("#", "Number of positional parameters"),
        ("0", "Script/shell name"),
        ("-", "Shell options"),
        ("$", "Shell process ID"),
        ("_", "Last command argument"),
    ];

    for (var_name, description) in special_vars {
        if var_name.starts_with(prefix) || prefix.is_empty() {
            completions.push(Completion {
                text: format!("${}", var_name),
                display: var_name.to_string(),
                description: Some(description.to_string()),
                is_dir: false,
            });
        }
    }

    // Add environment variables with values shown as descriptions
    let mut env_vars: Vec<_> = state.env_vars.keys().collect();
    env_vars.sort();
    for name in env_vars {
        if name.starts_with(prefix) || prefix.is_empty() {
            let value = state.env_vars.get(name).cloned().unwrap_or_default();
            // Show first 50 chars of value as description
            let desc = if value.len() > 50 {
                format!("{}...", &value[..50])
            } else {
                value
            };
            completions.push(Completion {
                text: format!("${}", name),
                display: name.clone(),
                description: Some(desc),
                is_dir: false,
            });
        }
    }

    // Add local variables from all scopes
    for scope in &state.local_vars_stack {
        let mut local_names: Vec<_> = scope.keys().collect();
        local_names.sort();
        for name in local_names {
            if name.starts_with(prefix) || prefix.is_empty() {
                completions.push(Completion {
                    text: format!("${}", name),
                    display: name.clone(),
                    description: Some("local".to_string()),
                    is_dir: false,
                });
            }
        }
    }

    // Add array names
    let mut array_names: Vec<_> = state.arrays.keys().collect();
    array_names.sort();
    for name in array_names {
        if name.starts_with(prefix) || prefix.is_empty() {
            let len = state.array_length(name);
            completions.push(Completion {
                text: format!("${{{}[@]}}", name),
                display: format!("{} [{}]", name, len),
                description: Some(format!("array ({} items)", len)),
                is_dir: false,
            });
        }
    }

    // Add associative array names
    let mut assoc_names: Vec<_> = state.assoc_arrays.keys().collect();
    assoc_names.sort();
    for name in assoc_names {
        if name.starts_with(prefix) || prefix.is_empty() {
            let len = state.array_length(name);
            completions.push(Completion {
                text: format!("${{{}[@]}}", name),
                display: format!("{} [{}]", name, len),
                description: Some(format!("assoc array ({} items)", len)),
                is_dir: false,
            });
        }
    }

    // Remove duplicates
    completions.dedup_by(|a, b| a.text == b.text);

    // Apply fuzzy filtering
    let filtered = filter_completions(completions, prefix);
    filtered.into_iter().take(50).collect()
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

/// Fuzzy match score: higher is better
/// 精确前缀匹配最高分，然后是首字母匹配，最后是子字符串匹配
pub fn fuzzy_match_score(text: &str, pattern: &str) -> i32 {
    if pattern.is_empty() {
        return 1000; // Empty pattern matches everything with high score
    }

    let text_lower = text.to_lowercase();
    let pattern_lower = pattern.to_lowercase();

    // Exact prefix match: highest score
    if text_lower.starts_with(&pattern_lower) {
        return 1000 - (text_lower.len() as i32 - pattern_lower.len() as i32).abs();
    }

    // Check if all characters of pattern exist in text in order
    let mut pattern_chars = pattern_lower.chars().peekable();
    let mut text_chars = text_lower.chars();
    let mut last_match_pos = 0;
    let mut match_count = 0;
    let mut gap_penalty = 0;

    for (pos, text_char) in text_chars.by_ref().enumerate() {
        if let Some(&pattern_char) = pattern_chars.peek() {
            if text_char == pattern_char {
                pattern_chars.next();
                match_count += 1;

                // Penalty for gaps between matches
                gap_penalty += pos.saturating_sub(last_match_pos).saturating_sub(1) as i32;
                last_match_pos = pos;

                // Bonus for consecutive matches
                if pos > 0 && text_lower.chars().nth(pos - 1)
                    .map(|c| c == pattern_lower.chars().next().unwrap())
                    .unwrap_or(false)
                {
                    gap_penalty = gap_penalty.saturating_sub(5);
                }
            }
        }
    }

    if match_count == pattern_lower.len() {
        // All characters matched, score based on gaps and position
        500 + (match_count as i32 * 10) - gap_penalty
    } else {
        0 // No match
    }
}

/// Filter completions using fuzzy matching
pub fn filter_completions(completions: Vec<Completion>, pattern: &str) -> Vec<Completion> {
    let mut scored: Vec<(Completion, i32)> = completions
        .into_iter()
        .map(|c| {
            let score = fuzzy_match_score(&c.text, pattern);
            (c, score)
        })
        .filter(|(_, score)| *score > 0)
        .collect();

    // Sort by score descending, then by text length (shorter is better)
    scored.sort_by(|a, b| {
        let score_cmp = b.1.cmp(&a.1);
        if score_cmp == std::cmp::Ordering::Equal {
            a.0.text.len().cmp(&b.0.text.len())
        } else {
            score_cmp
        }
    });

    scored.into_iter().map(|(c, _)| c).collect()
}

/// Clear the completion cache (useful for tests and cache invalidation)
pub fn clear_cache() {
    COMPLETION_CACHE.with(|cache| {
        cache.borrow_mut().clear();
    });
}

/// Complete history commands based on prefix
/// Returns a list of historical commands sorted by relevance
pub fn complete_from_history(prefix: &str) -> Vec<Completion> {
    let mut completions = Vec::new();

    // Try to load history file
    if let Ok(file) = std::fs::File::open(
        dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".rsh_history")
    ) {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(file);
        let mut seen = std::collections::HashSet::new();

        for line in reader.lines() {
            if let Ok(cmd_line) = line {
                let cmd = cmd_line.split_whitespace().next().unwrap_or("");

                // Avoid duplicates
                if seen.contains(cmd) {
                    continue;
                }
                seen.insert(cmd.to_string());

                if !cmd.is_empty() {
                    completions.push(Completion {
                        text: cmd.to_string(),
                        display: cmd.to_string(),
                        description: Some("history".to_string()),
                        is_dir: false,
                    });
                }
            }
        }
    }

    // Reverse to show most recent first, then filter
    completions.reverse();
    filter_completions(completions, prefix)
        .into_iter()
        .take(20)
        .collect()
}
