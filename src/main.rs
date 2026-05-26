#![allow(dead_code)]

mod ai;
mod bookmarks;
mod builtins;
mod completer;
mod completion_spec;
mod config;
mod workflows;
mod data;
mod debug;
mod editor;
mod environment;
mod executor;
mod expand;
mod glob_match;
mod highlighter;
mod history;
mod hooks;
mod job;
mod keybindings;
mod osc;
mod parser;
mod probe;
mod prompt;
pub mod session;
mod shell;
mod signal;
mod stream;
mod structured;
mod suggest;
mod zjump;

use crossterm::terminal;

/// Parse `--session <id>` from CLI args, falling back to RSH_SESSION_ID env var.
fn parse_session_id() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--session" {
            return args.get(i + 1).cloned();
        }
    }
    std::env::var("RSH_SESSION_ID").ok().filter(|s| !s.is_empty())
}

fn main() {
    // Fast path: non-interactive modes (-c, script) skip Editor/History entirely
    if let Some(code) = shell::run_noninteractive() {
        std::process::exit(code);
    }

    // Parse session ID for interactive mode
    let session_id = parse_session_id();

    // Interactive mode: set up panic hook to restore terminal
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        default_hook(info);
    }));

    let mut shell = shell::Shell::new();
    if let Some(ref id) = session_id {
        shell.restore_session(id);
    }
    shell.run();
}
