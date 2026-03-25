/// Prompt rendering with colors, git branch, exit code.

use crate::environment::ShellState;
use crossterm::style::Stylize;
use std::env;

pub fn render_prompt(state: &ShellState) -> String {
    let user = env::var("USER").unwrap_or_else(|_| String::from("user"));
    let hostname = get_hostname();
    let cwd = get_short_cwd(state);
    let git_branch = get_git_branch();
    let exit_indicator = if state.last_exit_code == 0 {
        "❯".green().bold().to_string()
    } else {
        "❯".red().bold().to_string()
    };

    let mut prompt = String::new();

    // User@host in bold blue
    prompt.push_str(&format!("{}", format!("{}@{}", user, hostname).dark_cyan().bold()));
    prompt.push(' ');

    // CWD in bold cyan
    prompt.push_str(&format!("{}", cwd.blue().bold()));

    // Git branch in magenta
    if let Some(branch) = &git_branch {
        prompt.push_str(&format!(" {}", format!("({})", branch).magenta()));
    }

    prompt.push(' ');
    prompt.push_str(&exit_indicator);
    prompt.push(' ');

    prompt
}

fn get_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| String::from("localhost"))
}

fn get_short_cwd(state: &ShellState) -> String {
    let cwd = env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| String::from("?"));

    let home = state.home_dir.to_string_lossy();
    if cwd.starts_with(home.as_ref()) {
        format!("~{}", &cwd[home.len()..])
    } else {
        cwd
    }
}

fn get_git_branch() -> Option<String> {
    // Walk up from current directory looking for .git
    let mut dir = env::current_dir().ok()?;
    loop {
        let git_head = dir.join(".git/HEAD");
        if git_head.exists() {
            let content = std::fs::read_to_string(git_head).ok()?;
            let content = content.trim();
            if let Some(branch) = content.strip_prefix("ref: refs/heads/") {
                return Some(branch.to_string());
            }
            // Detached HEAD
            return Some(content[..8.min(content.len())].to_string());
        }
        if !dir.pop() { break; }
    }
    None
}

/// Render the continuation prompt for multiline input.
pub fn render_continuation_prompt() -> String {
    format!("{} ", "> ".dark_grey())
}
