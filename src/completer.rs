/// Tab completion engine: context-aware completion for commands, paths, variables,
/// with configurable completion specs (Phase 7).
use crate::environment::ShellState;
use std::collections::HashMap;
use std::fs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionKind {
    Command,
    Builtin,
    Alias,
    Function,
    Directory,
    File,
    Variable,
    Subcommand,
    Flag,
    Other,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub display: String,
    pub description: Option<String>,
    pub kind: CompletionKind,
    pub is_dir: bool,
}

impl Completion {
    fn new(text: String, kind: CompletionKind) -> Self {
        let is_dir = kind == CompletionKind::Directory;
        Completion {
            display: text.clone(),
            text,
            description: None,
            kind,
            is_dir,
        }
    }

    fn with_desc(mut self, desc: &str) -> Self {
        self.description = Some(desc.to_string());
        self
    }
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
            if let Some(lfu_key) = self
                .cache
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
        // Argument completion depends on the full command and repository
        // context, not just the last word (which is often empty after a space).
        format!(
            "arg:{}:{}:{}:{}:{}",
            cmd,
            &buf[..word_start],
            word,
            state.cached_git_branch.as_deref().unwrap_or(""),
            state.cached_git_remote.as_deref().unwrap_or("")
        )
    };

    // Try to get from cache
    let cached = COMPLETION_CACHE.with(|cache| cache.borrow_mut().get(&cache_key));

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

    // Detect if we're after a pipe for smart recommendations
    let after_pipe = {
        let before = buf[..word_start].trim_end();
        before.ends_with('|')
    };

    let completions = if word.starts_with('$') {
        complete_variable(&word[1..], state)
    } else if is_cmd_pos && after_pipe {
        // Smart pipe completion: recommend based on preceding command
        let mut pipe_completions = complete_pipe_targets(buf, &word);
        if pipe_completions.is_empty() {
            complete_command(&word, state)
        } else {
            // Also include regular command completions after pipe suggestions
            let mut regular = complete_command(&word, state);
            pipe_completions.append(&mut regular);
            pipe_completions
        }
    } else if is_cmd_pos {
        let mut cmd_completions = complete_command(&word, state);
        // Append project-aware completions for short prefixes
        if word.len() <= 3 {
            let project = complete_project_commands(&word);
            cmd_completions.extend(project);
        }
        cmd_completions
    } else if let Some(subs) = subcommand_completions(&cmd, &word, buf, word_start, state) {
        subs
    } else if let Some(spec_completions) = complete_from_spec(&cmd, &word, buf, state) {
        spec_completions
    } else if cmd == "cd" || cmd == "mkdir" || cmd == "rmdir" || cmd == "z" {
        complete_path(&word, state)
            .into_iter()
            .filter(|c| c.is_dir)
            .collect()
    } else {
        complete_path(&word, state)
    };

    // Store in cache
    COMPLETION_CACHE.with(|cache| {
        cache.borrow_mut().insert(cache_key, completions.clone());
    });

    (word_start, completions)
}

fn apply_completion_spec(
    spec: &crate::environment::CompletionSpec,
    prefix: &str,
    state: &mut ShellState,
) -> Vec<Completion> {
    let mut completions = Vec::new();

    // -W word list
    if let Some(ref words) = spec.word_list {
        for w in words {
            if w.starts_with(prefix) {
                completions.push(Completion {
                    text: w.clone(),
                    display: w.clone(),
                    description: None,
                    kind: CompletionKind::Other,
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
                scope.insert(
                    "COMP_CWORD".to_string(),
                    (words.len().saturating_sub(1)).to_string(),
                );
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
                            kind: CompletionKind::Other,
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
        completions.extend(
            complete_path(prefix, state)
                .into_iter()
                .filter(|c| c.is_dir),
        );
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
        for c in &mut completions {
            c.text = format!("{}{}", pfx, c.text);
        }
    }
    if let Some(ref sfx) = spec.suffix {
        for c in &mut completions {
            c.text = format!("{}{}", c.text, sfx);
        }
    }

    completions
}

fn first_command(buf: &str) -> String {
    let trimmed = buf.trim_start();
    let cmd_start = trimmed
        .rfind(|c: char| c == '|' || c == ';')
        .map(|i| i + 1)
        .unwrap_or(0);
    let segment = trimmed[cmd_start..].trim_start();
    segment.split_whitespace().next().unwrap_or("").to_string()
}

fn subcommand_completions(
    cmd: &str,
    prefix: &str,
    buf: &str,
    word_start: usize,
    state: &ShellState,
) -> Option<Vec<Completion>> {
    let before = buf[..word_start].trim_end();
    let words: Vec<&str> = before.split_whitespace().collect();
    let word_count = words.len();

    // First-level subcommands with descriptions
    if word_count == 1 {
        let subs: &[(&str, &str)] = match cmd {
            "git" => &[
                ("add", "Stage changes"),
                ("bisect", "Binary search for bugs"),
                ("blame", "Show line annotations"),
                ("branch", "List/create branches"),
                ("checkout", "Switch branches/restore files"),
                ("cherry-pick", "Apply commit changes"),
                ("clone", "Clone a repository"),
                ("commit", "Record changes"),
                ("config", "Get/set configuration"),
                ("diff", "Show changes"),
                ("fetch", "Download objects/refs"),
                ("grep", "Search tracked files"),
                ("init", "Create empty repository"),
                ("log", "Show commit log"),
                ("merge", "Join branches"),
                ("mv", "Move/rename files"),
                ("pull", "Fetch and merge"),
                ("push", "Update remote refs"),
                ("rebase", "Reapply commits"),
                ("remote", "Manage remotes"),
                ("reset", "Reset HEAD"),
                ("restore", "Restore working tree"),
                ("revert", "Revert commits"),
                ("rm", "Remove files"),
                ("show", "Show objects"),
                ("stash", "Stash changes"),
                ("status", "Show working tree status"),
                ("switch", "Switch branches"),
                ("tag", "Manage tags"),
                ("worktree", "Manage worktrees"),
            ],
            "cargo" => &[
                ("add", "Add dependency"),
                ("bench", "Run benchmarks"),
                ("build", "Compile project"),
                ("check", "Check for errors"),
                ("clean", "Remove artifacts"),
                ("clippy", "Run linter"),
                ("doc", "Build documentation"),
                ("fetch", "Fetch dependencies"),
                ("fix", "Auto-fix warnings"),
                ("fmt", "Format code"),
                ("init", "Init in existing dir"),
                ("install", "Install binary"),
                ("new", "Create new project"),
                ("publish", "Publish to crates.io"),
                ("remove", "Remove dependency"),
                ("run", "Run binary"),
                ("search", "Search crates.io"),
                ("test", "Run tests"),
                ("tree", "Show dependency tree"),
                ("uninstall", "Remove binary"),
                ("update", "Update dependencies"),
            ],
            "docker" => &[
                ("build", "Build image"),
                ("compose", "Multi-container apps"),
                ("container", "Manage containers"),
                ("cp", "Copy files"),
                ("create", "Create container"),
                ("exec", "Run in container"),
                ("image", "Manage images"),
                ("images", "List images"),
                ("kill", "Kill container"),
                ("logs", "View logs"),
                ("network", "Manage networks"),
                ("ps", "List containers"),
                ("pull", "Pull image"),
                ("push", "Push image"),
                ("restart", "Restart container"),
                ("rm", "Remove container"),
                ("rmi", "Remove image"),
                ("run", "Create and run"),
                ("start", "Start container"),
                ("stop", "Stop container"),
                ("tag", "Tag image"),
                ("volume", "Manage volumes"),
            ],
            "systemctl" => &[
                ("daemon-reload", "Reload unit files"),
                ("disable", "Disable unit"),
                ("edit", "Edit unit file"),
                ("enable", "Enable unit"),
                ("is-active", "Check if active"),
                ("is-enabled", "Check if enabled"),
                ("list-units", "List loaded units"),
                ("reload", "Reload unit"),
                ("restart", "Restart unit"),
                ("start", "Start unit"),
                ("status", "Show status"),
                ("stop", "Stop unit"),
            ],
            "npm" => &[
                ("audit", "Security audit"),
                ("build", "Build package"),
                ("cache", "Manage cache"),
                ("ci", "Clean install"),
                ("clean", "Clean project"),
                ("config", "Manage config"),
                ("create", "Create package"),
                ("exec", "Run package binary"),
                ("init", "Init package.json"),
                ("install", "Install packages"),
                ("link", "Symlink package"),
                ("list", "List installed"),
                ("outdated", "Check outdated"),
                ("pack", "Create tarball"),
                ("publish", "Publish package"),
                ("rebuild", "Rebuild native"),
                ("remove", "Remove package"),
                ("run", "Run script"),
                ("search", "Search registry"),
                ("start", "Start script"),
                ("test", "Run tests"),
                ("uninstall", "Uninstall package"),
                ("update", "Update packages"),
                ("version", "Bump version"),
            ],
            "hook" => &[
                ("add", "Add hook"),
                ("remove", "Remove hook"),
                ("list", "List hooks"),
            ],
            "bookmark" => &[
                ("add", "Add bookmark"),
                ("go", "Go to bookmark"),
                ("ls", "List bookmarks"),
                ("rm", "Remove bookmark"),
            ],
            "kubectl" => &[
                ("apply", "Apply configuration"),
                ("attach", "Attach to container"),
                ("auth", "Check authorization"),
                ("config", "Modify kubeconfig"),
                ("create", "Create resource"),
                ("delete", "Delete resources"),
                ("describe", "Show resource details"),
                ("diff", "Diff configurations"),
                ("edit", "Edit resource"),
                ("exec", "Execute in container"),
                ("expose", "Expose as service"),
                ("get", "Display resources"),
                ("label", "Update labels"),
                ("logs", "Print container logs"),
                ("patch", "Patch resource"),
                ("port-forward", "Forward ports"),
                ("proxy", "Run API proxy"),
                ("rollout", "Manage rollouts"),
                ("run", "Run pod"),
                ("scale", "Scale replicas"),
                ("set", "Set resource fields"),
                ("top", "Resource usage"),
                ("version", "Print version"),
            ],
            "pip" | "pip3" => &[
                ("install", "Install packages"),
                ("uninstall", "Uninstall packages"),
                ("download", "Download packages"),
                ("freeze", "Output installed"),
                ("list", "List installed"),
                ("show", "Show package info"),
                ("search", "Search PyPI"),
                ("wheel", "Build wheels"),
                ("hash", "Compute hashes"),
                ("check", "Verify packages"),
                ("config", "Manage config"),
                ("cache", "Manage cache"),
            ],
            "go" => &[
                ("build", "Compile packages"),
                ("clean", "Remove objects"),
                ("doc", "Show documentation"),
                ("env", "Print environment"),
                ("fix", "Update packages"),
                ("fmt", "Format source"),
                ("generate", "Run go generate"),
                ("get", "Download modules"),
                ("install", "Compile and install"),
                ("list", "List packages"),
                ("mod", "Module maintenance"),
                ("run", "Compile and run"),
                ("test", "Run tests"),
                ("tool", "Run go tool"),
                ("version", "Print version"),
                ("vet", "Report issues"),
                ("work", "Workspace mode"),
            ],
            // Phase 14d: signature-driven first-arg completion for
            // `help <cmd>` — list every signed value-aware builtin.
            "help" => {
                let mut names: Vec<&'static str> =
                    crate::signature::SIGNATURES.keys().copied().collect();
                names.sort_unstable();
                let completions: Vec<Completion> = names
                    .into_iter()
                    .filter(|n| n.starts_with(prefix))
                    .map(|n| {
                        let sig = crate::signature::SIGNATURES.get(n).unwrap();
                        Completion {
                            text: n.to_string(),
                            display: n.to_string(),
                            description: Some(sig.desc.to_string()),
                            kind: CompletionKind::Subcommand,
                            is_dir: false,
                        }
                    })
                    .collect();
                return Some(completions);
            }
            // `error <subcmd>` — currently just `make`.
            "error" => {
                let subs = [("make", "Raise a structured error with a message")];
                let completions: Vec<Completion> = subs
                    .iter()
                    .filter(|(n, _)| n.starts_with(prefix))
                    .map(|(n, d)| Completion {
                        text: n.to_string(),
                        display: n.to_string(),
                        description: Some(d.to_string()),
                        kind: CompletionKind::Subcommand,
                        is_dir: false,
                    })
                    .collect();
                return Some(completions);
            }
            _ => return None,
        };

        let completions = subs
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(name, desc)| Completion {
                text: name.to_string(),
                display: name.to_string(),
                description: Some(desc.to_string()),
                kind: CompletionKind::Subcommand,
                is_dir: false,
            })
            .collect::<Vec<_>>();

        return Some(completions);
    }

    // Second-level: git context-aware completions
    if cmd == "git" && word_count >= 2 {
        let subcmd = words.get(1).copied().unwrap_or("");
        match subcmd {
            "checkout" | "switch" | "merge" | "rebase" | "branch" | "diff" | "log" => {
                return Some(complete_git_refs(prefix));
            }
            "add" => {
                return Some(complete_git_dirty_files(prefix, "add"));
            }
            "restore" => {
                return Some(complete_git_dirty_files(prefix, "restore"));
            }
            "reset" => {
                let mut results = complete_git_refs(prefix);
                results.extend(complete_git_dirty_files(prefix, "reset"));
                return Some(results);
            }
            "stash" if word_count == 2 => {
                // stash subcommands
                let subs = &[
                    ("push", "Stash changes"),
                    ("pop", "Apply and drop"),
                    ("apply", "Apply stash"),
                    ("drop", "Drop stash"),
                    ("list", "List stashes"),
                    ("show", "Show stash"),
                    ("clear", "Clear all stashes"),
                ];
                let completions = subs
                    .iter()
                    .filter(|(name, _)| name.starts_with(prefix))
                    .map(|(name, desc)| Completion {
                        text: name.to_string(),
                        display: name.to_string(),
                        description: Some(desc.to_string()),
                        kind: CompletionKind::Subcommand,
                        is_dir: false,
                    })
                    .collect();
                return Some(completions);
            }
            "stash" if word_count >= 3 => {
                let stash_sub = words.get(2).copied().unwrap_or("");
                if stash_sub == "pop"
                    || stash_sub == "apply"
                    || stash_sub == "drop"
                    || stash_sub == "show"
                {
                    return Some(complete_git_stashes(prefix));
                }
            }
            "cherry-pick" | "revert" => {
                return Some(complete_git_recent_commits(prefix));
            }
            "remote" if word_count == 2 => {
                let subs = &[
                    ("add", "Add remote"),
                    ("remove", "Remove remote"),
                    ("rename", "Rename remote"),
                    ("show", "Show remote"),
                    ("prune", "Prune stale refs"),
                    ("update", "Fetch updates"),
                ];
                let completions = subs
                    .iter()
                    .filter(|(name, _)| name.starts_with(prefix))
                    .map(|(name, desc)| Completion {
                        text: name.to_string(),
                        display: name.to_string(),
                        description: Some(desc.to_string()),
                        kind: CompletionKind::Subcommand,
                        is_dir: false,
                    })
                    .collect();
                return Some(completions);
            }
            "remote" if word_count >= 3 => {
                return Some(complete_git_remotes(prefix));
            }
            "push" | "pull" | "fetch" if word_count == 2 => {
                let mut results = complete_git_remotes(prefix);
                if let Some(remote) = state.cached_git_remote.as_deref() {
                    promote_git_context(&mut results, remote, prefix, "tracking remote");
                }
                return Some(results);
            }
            "push" | "pull" | "fetch" if word_count >= 3 => {
                let mut results = complete_git_refs(prefix);
                if let Some(branch) = state.cached_git_branch.as_deref() {
                    promote_git_context(&mut results, branch, prefix, "current branch");
                }
                return Some(results);
            }
            _ => {}
        }
    }

    // Second-level: docker compose subcommands
    if cmd == "docker" && word_count == 2 {
        let subcmd = words.get(1).copied().unwrap_or("");
        if subcmd == "compose" {
            let subs = &[
                "build", "config", "create", "down", "events", "exec", "images", "kill", "logs",
                "ls", "pause", "port", "ps", "pull", "push", "restart", "rm", "run", "start",
                "stop", "top", "unpause", "up",
            ];
            let completions = subs
                .iter()
                .filter(|s| s.starts_with(prefix))
                .map(|s| Completion {
                    text: s.to_string(),
                    display: s.to_string(),
                    description: None,
                    kind: CompletionKind::Subcommand,
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
                let completions = db
                    .names()
                    .into_iter()
                    .filter(|n| n.starts_with(prefix))
                    .map(|n| Completion {
                        text: n.clone(),
                        display: n,
                        description: Some("bookmark".to_string()),
                        kind: CompletionKind::Other,
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
            "mkdir" => &[("-p", "parents"), ("-m", "mode"), ("-v", "verbose")],
            "chmod" => &[
                ("-r", "recursive"),
                ("-v", "verbose"),
                ("-c", "changes only"),
                ("-R", "recursive"),
            ],
            _ => return None,
        };

        let completions = options
            .iter()
            .filter(|(opt, _)| opt.starts_with(prefix))
            .map(|(opt, desc)| Completion {
                text: opt.to_string(),
                display: opt.to_string(),
                description: Some(desc.to_string()),
                kind: CompletionKind::Flag,
                is_dir: false,
            })
            .collect::<Vec<_>>();

        if !completions.is_empty() {
            return Some(completions);
        }
    }

    None
}

fn promote_git_context(
    completions: &mut Vec<Completion>,
    value: &str,
    prefix: &str,
    description: &str,
) {
    if !value.starts_with(prefix) {
        return;
    }
    if let Some(index) = completions
        .iter()
        .position(|completion| completion.text == value)
    {
        let mut completion = completions.remove(index);
        completion.description = Some(description.to_string());
        completions.insert(0, completion);
    } else {
        completions.insert(
            0,
            Completion {
                text: value.to_string(),
                display: value.to_string(),
                description: Some(description.to_string()),
                kind: CompletionKind::Other,
                is_dir: false,
            },
        );
    }
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
                        kind: CompletionKind::Other,
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
                        kind: CompletionKind::Other,
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
                            kind: CompletionKind::Other,
                            is_dir: false,
                        });
                    }
                }
            }
        }
    }

    completions
}

fn complete_git_dirty_files(prefix: &str, context: &str) -> Vec<Completion> {
    let mut completions = Vec::new();
    if let Ok(output) = std::process::Command::new("git")
        .args(["status", "--porcelain=v1"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.len() < 4 {
                    continue;
                }
                let status = &line[..2];
                let file = line[3..].trim_start();
                // Strip quotes from filenames with special chars
                let file = file.trim_matches('"');

                if !file.starts_with(prefix) {
                    continue;
                }

                let (desc, include) = match context {
                    "add" => match status.trim() {
                        "M" | "MM" => ("modified", true),
                        "A" | "AM" => ("new file", true),
                        "D" => ("deleted", true),
                        "R" => ("renamed", true),
                        "??" => ("untracked", true),
                        _ => ("", true),
                    },
                    "restore" => match status.chars().next().unwrap_or(' ') {
                        'M' | 'A' | 'D' => ("staged", true),
                        _ => match status.chars().nth(1).unwrap_or(' ') {
                            'M' | 'D' => ("modified", true),
                            _ => ("", false),
                        },
                    },
                    "reset" => match status.chars().next().unwrap_or(' ') {
                        'M' | 'A' | 'D' | 'R' => ("staged", true),
                        _ => ("", false),
                    },
                    _ => ("", true),
                };

                if include {
                    completions.push(Completion {
                        text: file.to_string(),
                        display: file.to_string(),
                        description: Some(desc.to_string()),
                        kind: CompletionKind::File,
                        is_dir: false,
                    });
                }
            }
        }
    }
    completions
}

fn complete_git_stashes(prefix: &str) -> Vec<Completion> {
    let mut completions = Vec::new();
    if let Ok(output) = std::process::Command::new("git")
        .args(["stash", "list", "--format=%gd|%gs"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.splitn(2, '|').collect();
                let (ref_name, msg) = if parts.len() == 2 {
                    (parts[0], parts[1])
                } else {
                    (line, "")
                };
                if ref_name.starts_with(prefix) || prefix.is_empty() {
                    completions.push(Completion {
                        text: ref_name.to_string(),
                        display: ref_name.to_string(),
                        description: Some(msg.to_string()),
                        kind: CompletionKind::Other,
                        is_dir: false,
                    });
                }
            }
        }
    }
    completions
}

fn complete_git_recent_commits(prefix: &str) -> Vec<Completion> {
    let mut completions = Vec::new();
    if let Ok(output) = std::process::Command::new("git")
        .args(["log", "--oneline", "-20", "--format=%h|%s"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.splitn(2, '|').collect();
                let (hash, msg) = if parts.len() == 2 {
                    (parts[0], parts[1])
                } else {
                    (line, "")
                };
                if hash.starts_with(prefix) || prefix.is_empty() {
                    let desc = if msg.len() > 40 {
                        format!("{}…", &msg[..39])
                    } else {
                        msg.to_string()
                    };
                    completions.push(Completion {
                        text: hash.to_string(),
                        display: hash.to_string(),
                        description: Some(desc),
                        kind: CompletionKind::Other,
                        is_dir: false,
                    });
                }
            }
        }
    }
    completions
}

fn complete_git_remotes(prefix: &str) -> Vec<Completion> {
    let mut completions = Vec::new();
    if let Ok(output) = std::process::Command::new("git").args(["remote"]).output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for remote in stdout.lines() {
                let remote = remote.trim();
                if !remote.is_empty() && remote.starts_with(prefix) {
                    completions.push(Completion {
                        text: remote.to_string(),
                        display: remote.to_string(),
                        description: Some("remote".to_string()),
                        kind: CompletionKind::Other,
                        is_dir: false,
                    });
                }
            }
        }
    }
    completions
}

fn complete_from_spec(
    cmd: &str,
    prefix: &str,
    buf: &str,
    state: &ShellState,
) -> Option<Vec<Completion>> {
    use crate::completion_spec::SpecCompletionKind;

    let words: Vec<&str> = buf.split_whitespace().collect();
    let ctx = state.spec_registry.resolve_context(cmd, &words)?;

    let results = ctx.complete_prefix(prefix);
    if results.is_empty() {
        return None;
    }

    let completions = results
        .into_iter()
        .map(|(text, desc, kind)| {
            let ck = match kind {
                SpecCompletionKind::Subcommand => CompletionKind::Subcommand,
                SpecCompletionKind::Option => CompletionKind::Flag,
                SpecCompletionKind::Argument => CompletionKind::Other,
            };
            Completion {
                display: text.clone(),
                text,
                description: desc,
                kind: ck,
                is_dir: false,
            }
        })
        .collect();

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
                if i == 0 {
                    start = 0;
                }
            }
        }
    }
    let word = buf[start..].to_string();
    (word, start)
}

fn is_command_position(buf: &str, word_start: usize) -> bool {
    let before = buf[..word_start].trim_end();
    before.is_empty()
        || before.ends_with('|')
        || before.ends_with("&&")
        || before.ends_with("||")
        || before.ends_with(';')
        || before.ends_with('(')
        || before.ends_with('{')
}

fn complete_command(prefix: &str, state: &mut ShellState) -> Vec<Completion> {
    let mut completions = Vec::new();

    // Collect all builtin commands
    for cmd in crate::builtins::BUILTIN_NAMES {
        completions.push(Completion {
            text: cmd.to_string(),
            display: cmd.to_string(),
            description: Some("builtin".to_string()),
            kind: CompletionKind::Builtin,
            is_dir: false,
        });
    }

    // Phase 14d: surface signed value-aware builtins (try/each/where/...).
    // Description carries the input → output signature so users can pick the
    // right command by type from the completion list.
    for (name, sig) in crate::signature::SIGNATURES.iter() {
        let desc = format!("{} → {}", sig.input.render(), sig.output.render());
        completions.push(Completion {
            text: (*name).to_string(),
            display: (*name).to_string(),
            description: Some(desc),
            kind: CompletionKind::Builtin,
            is_dir: false,
        });
    }

    // Collect aliases
    for name in state.aliases.keys() {
        completions.push(Completion {
            text: name.clone(),
            display: name.clone(),
            description: Some("alias".to_string()),
            kind: CompletionKind::Alias,
            is_dir: false,
        });
    }

    // Collect functions
    for name in state.functions.keys() {
        completions.push(Completion {
            text: name.clone(),
            display: name.clone(),
            description: Some("function".to_string()),
            kind: CompletionKind::Function,
            is_dir: false,
        });
    }

    // Phase 15c: typed user functions registered via `def`. Description shows
    // the parameter sketch (e.g. "a:int b:string") so completions are useful.
    for (name, sig) in state.user_signatures.iter() {
        let desc = if sig.params.is_empty() {
            "user-defined".to_string()
        } else {
            sig.params
                .iter()
                .map(|p| {
                    format!(
                        "{}{}{}",
                        p.name,
                        if p.optional {
                            "?"
                        } else if p.rest {
                            "..."
                        } else {
                            ""
                        },
                        if matches!(p.kind, crate::signature::Type::Any) {
                            String::new()
                        } else {
                            format!(":{}", p.kind.render())
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        completions.push(Completion {
            text: name.clone(),
            display: name.clone(),
            description: Some(desc),
            kind: CompletionKind::Function,
            is_dir: false,
        });
    }

    // Collect commands in PATH
    for cmd in state.path_cache().iter() {
        completions.push(Completion {
            text: cmd.clone(),
            display: cmd.clone(),
            description: None,
            kind: CompletionKind::Command,
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

fn path_metadata_desc(entry: &fs::DirEntry) -> Option<String> {
    let ft = entry.file_type().ok()?;
    if ft.is_symlink() {
        let target = fs::read_link(entry.path()).ok()?;
        return Some(format!("→ {}", target.display()));
    }
    if ft.is_dir() {
        let count = fs::read_dir(entry.path()).ok()?.count();
        return Some(format!("{} items", count));
    }
    if ft.is_file() {
        let meta = entry.metadata().ok()?;
        let size = meta.len();
        return Some(format_file_size(size));
    }
    None
}

fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{}B", bytes);
    }
    if bytes < 1024 * 1024 {
        return format!("{:.1}K", bytes as f64 / 1024.0);
    }
    if bytes < 1024 * 1024 * 1024 {
        return format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0));
    }
    format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
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
            if !name.starts_with(file_prefix) {
                continue;
            }
            if name.starts_with('.') && !file_prefix.starts_with('.') {
                continue;
            }

            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let full = if dir == "." {
                if is_dir {
                    format!("{}/", name)
                } else {
                    name.clone()
                }
            } else if prefix.starts_with('~') {
                let suffix = if expanded.ends_with('/') {
                    format!("{}{}", &prefix, name)
                } else {
                    match prefix.rfind('/') {
                        Some(pos) => format!("{}/{}", &prefix[..pos], name),
                        None => format!("~/{}", name),
                    }
                };
                if is_dir {
                    format!("{}/", suffix)
                } else {
                    suffix
                }
            } else {
                let path = if expanded.ends_with('/') {
                    format!("{}{}", prefix, name)
                } else {
                    match prefix.rfind('/') {
                        Some(pos) => format!("{}/{}", &prefix[..pos], name),
                        None => name.clone(),
                    }
                };
                if is_dir {
                    format!("{}/", path)
                } else {
                    path
                }
            };

            let description = path_metadata_desc(&entry);

            completions.push(Completion {
                text: full,
                display: if is_dir {
                    format!("{}/", name)
                } else {
                    name.clone()
                },
                description,
                kind: if is_dir {
                    CompletionKind::Directory
                } else {
                    CompletionKind::File
                },
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
                kind: CompletionKind::Variable,
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
            let desc = if value.len() > 50 {
                format!("{}...", &value[..50])
            } else {
                value
            };
            completions.push(Completion {
                text: format!("${}", name),
                display: name.clone(),
                description: Some(desc),
                kind: CompletionKind::Variable,
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
                    kind: CompletionKind::Variable,
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
                kind: CompletionKind::Variable,
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
                kind: CompletionKind::Variable,
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
    if completions.is_empty() {
        return String::new();
    }
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
                if pos > 0
                    && text_lower
                        .chars()
                        .nth(pos - 1)
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
            .join(".rsh_history"),
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
                        kind: CompletionKind::Command,
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

/// Smart pipe completion: recommend pipe targets based on preceding command
pub fn complete_pipe_targets(buf: &str, prefix: &str) -> Vec<Completion> {
    let before_pipe = buf.rsplitn(2, '|').nth(1).unwrap_or("").trim();
    let prev_cmd = before_pipe.split_whitespace().next().unwrap_or("");
    let prev_cmd_base = prev_cmd.rsplit('/').next().unwrap_or(prev_cmd);

    let suggestions: &[(&str, &str)] = match prev_cmd_base {
        "cat" | "less" | "head" | "tail" => &[
            ("grep", "Filter lines by pattern"),
            ("wc", "Count lines/words/bytes"),
            ("sort", "Sort lines"),
            ("uniq", "Remove duplicates"),
            ("awk", "Text processing"),
            ("sed", "Stream editing"),
            ("cut", "Extract columns"),
            ("tr", "Translate characters"),
        ],
        "curl" | "wget" => &[
            ("jq", "JSON processor"),
            ("grep", "Filter output"),
            ("python3 -m json.tool", "Pretty-print JSON"),
            ("tee", "Write and pass through"),
        ],
        "find" => &[
            ("xargs", "Execute on results"),
            ("grep", "Filter results"),
            ("sort", "Sort results"),
            ("wc -l", "Count results"),
            ("head", "First N results"),
        ],
        "ps" => &[
            ("grep", "Filter processes"),
            ("awk", "Extract columns"),
            ("sort", "Sort output"),
            ("head", "Top entries"),
        ],
        "ls" | "dir" => &[
            ("grep", "Filter files"),
            ("sort", "Sort output"),
            ("wc -l", "Count entries"),
            ("head", "First entries"),
        ],
        "docker" => &[
            ("grep", "Filter output"),
            ("awk", "Extract fields"),
            ("jq", "JSON processing"),
            ("xargs", "Execute on results"),
        ],
        "echo" | "printf" => &[
            ("tr", "Translate characters"),
            ("sed", "Stream editing"),
            ("base64", "Encode/decode"),
            ("xclip", "Copy to clipboard"),
        ],
        "git" => &[
            ("grep", "Filter output"),
            ("head", "First N lines"),
            ("wc -l", "Count lines"),
            ("sort", "Sort output"),
        ],
        "df" | "du" => &[
            ("sort -h", "Sort by size"),
            ("grep", "Filter output"),
            ("tail", "Last entries"),
            ("awk", "Extract columns"),
        ],
        _ => &[
            ("grep", "Filter by pattern"),
            ("sort", "Sort output"),
            ("head", "First N lines"),
            ("tail", "Last N lines"),
            ("wc", "Count lines/words"),
            ("awk", "Text processing"),
            ("xargs", "Execute on each line"),
            ("tee", "Write and pass through"),
        ],
    };

    let mut completions = Vec::new();
    for &(cmd, desc) in suggestions {
        if prefix.is_empty() || cmd.starts_with(prefix) {
            completions.push(Completion {
                text: cmd.to_string(),
                display: cmd.to_string(),
                description: Some(desc.to_string()),
                kind: CompletionKind::Command,
                is_dir: false,
            });
        }
    }
    completions
}

/// Detect project type and provide context-aware completions
pub fn complete_project_commands(prefix: &str) -> Vec<Completion> {
    let mut completions = Vec::new();
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(_) => return completions,
    };

    // Cargo.toml → Rust project
    if cwd.join("Cargo.toml").exists() {
        let rust_cmds: &[(&str, &str)] = &[
            ("cargo build", "Build the project"),
            ("cargo test", "Run tests"),
            ("cargo run", "Run the project"),
            ("cargo check", "Check for errors"),
            ("cargo clippy", "Run linter"),
            ("cargo fmt", "Format code"),
            ("cargo doc", "Build documentation"),
            ("cargo bench", "Run benchmarks"),
        ];
        for &(cmd, desc) in rust_cmds {
            if prefix.is_empty() || cmd.starts_with(prefix) {
                completions.push(Completion {
                    text: cmd.to_string(),
                    display: cmd.to_string(),
                    description: Some(desc.to_string()),
                    kind: CompletionKind::Command,
                    is_dir: false,
                });
            }
        }
    }

    // package.json → Node project
    if cwd.join("package.json").exists() {
        if let Ok(content) = fs::read_to_string(cwd.join("package.json")) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(scripts) = json["scripts"].as_object() {
                    for (name, val) in scripts {
                        let cmd = format!("npm run {}", name);
                        if prefix.is_empty() || cmd.starts_with(prefix) {
                            let desc = val.as_str().unwrap_or("").to_string();
                            completions.push(Completion {
                                text: cmd,
                                display: format!("npm run {}", name),
                                description: Some(desc),
                                kind: CompletionKind::Command,
                                is_dir: false,
                            });
                        }
                    }
                }
            }
        }
    }

    // Makefile → Make targets
    let makefile = if cwd.join("Makefile").exists() {
        Some(cwd.join("Makefile"))
    } else if cwd.join("makefile").exists() {
        Some(cwd.join("makefile"))
    } else if cwd.join("GNUmakefile").exists() {
        Some(cwd.join("GNUmakefile"))
    } else {
        None
    };
    if let Some(mf_path) = makefile {
        if let Ok(content) = fs::read_to_string(mf_path) {
            for line in content.lines() {
                if let Some(target) = line.strip_suffix(':') {
                    let target = target.split_whitespace().next().unwrap_or("");
                    if !target.is_empty() && !target.starts_with('.') && !target.contains('%') {
                        let cmd = format!("make {}", target);
                        if prefix.is_empty() || cmd.starts_with(prefix) {
                            completions.push(Completion {
                                text: cmd,
                                display: format!("make {}", target),
                                description: Some("Makefile target".to_string()),
                                kind: CompletionKind::Command,
                                is_dir: false,
                            });
                        }
                    }
                }
            }
        }
    }

    // pyproject.toml or setup.py → Python project
    if cwd.join("pyproject.toml").exists() || cwd.join("setup.py").exists() {
        let py_cmds: &[(&str, &str)] = &[
            ("python -m pytest", "Run tests"),
            ("pip install -e .", "Install in dev mode"),
            ("python -m mypy .", "Type check"),
            ("python -m black .", "Format code"),
        ];
        for &(cmd, desc) in py_cmds {
            if prefix.is_empty() || cmd.starts_with(prefix) {
                completions.push(Completion {
                    text: cmd.to_string(),
                    display: cmd.to_string(),
                    description: Some(desc.to_string()),
                    kind: CompletionKind::Command,
                    is_dir: false,
                });
            }
        }
    }

    // go.mod → Go project
    if cwd.join("go.mod").exists() {
        let go_cmds: &[(&str, &str)] = &[
            ("go build ./...", "Build all packages"),
            ("go test ./...", "Run all tests"),
            ("go run .", "Run the project"),
            ("go vet ./...", "Check for issues"),
            ("go mod tidy", "Clean up dependencies"),
        ];
        for &(cmd, desc) in go_cmds {
            if prefix.is_empty() || cmd.starts_with(prefix) {
                completions.push(Completion {
                    text: cmd.to_string(),
                    display: cmd.to_string(),
                    description: Some(desc.to_string()),
                    kind: CompletionKind::Command,
                    is_dir: false,
                });
            }
        }
    }

    completions
}
