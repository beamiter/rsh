/// Prompt rendering with colors, git branch, exit code.

use crate::environment::ShellState;
use crossterm::style::{Color, Stylize};
use std::env;

pub fn render_prompt(state: &ShellState) -> String {
    let user = env::var("USER").unwrap_or_else(|_| String::from("user"));
    let hostname = &state.hostname;
    let cwd = get_short_cwd(state);
    let git_branch = get_git_branch();
    let exit_indicator = if state.last_exit_code == 0 {
        "❯".green().bold().to_string()
    } else {
        "❯".red().bold().to_string()
    };

    let mut prompt = String::new();

    // User@host
    prompt.push_str(&format!("{}", format!("{}@{}", user, hostname)
        .with(Color::Rgb { r: 0, g: 210, b: 210 }).bold()));
    prompt.push(' ');

    // CWD
    prompt.push_str(&format!("{}", cwd
        .with(Color::Rgb { r: 80, g: 255, b: 120 }).bold()));

    // Git branch in magenta
    if let Some(branch) = &git_branch {
        prompt.push_str(&format!(" {}", format!("({})", branch).magenta()));
    }

    // Command duration
    if let Some(d) = &state.last_command_duration {
        let dur_str = format_duration(*d);
        if !dur_str.is_empty() {
            prompt.push_str(&format!(" {}", format!("took {}", dur_str)
                .with(Color::Yellow)));
        }
    }

    prompt.push(' ');
    prompt.push_str(&exit_indicator);
    prompt.push(' ');

    prompt
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

/// Render the right prompt (command duration).
pub fn render_rprompt(state: &ShellState) -> String {
    let mut rprompt = String::new();
    if let Some(d) = &state.last_command_duration {
        let dur_str = format_duration(*d);
        if !dur_str.is_empty() {
            rprompt.push_str(&format!("{}", dur_str.with(Color::Yellow)));
        }
    }
    rprompt
}

/// Get the raw (no ANSI) width of the rprompt for positioning.
pub fn rprompt_width(state: &ShellState) -> usize {
    if let Some(d) = &state.last_command_duration {
        let dur_str = format_duration(*d);
        dur_str.len()
    } else {
        0
    }
}

pub fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        format!("{}h{}m{}s", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else if secs >= 1 {
        format!("{:.1}s", d.as_secs_f64())
    } else {
        return String::new(); // Don't show for sub-second
    }
}
