#![allow(dead_code)]

mod bookmarks;
mod builtins;
mod completer;
mod config;
mod editor;
mod environment;
mod executor;
mod expand;
mod glob_match;
mod highlighter;
mod history;
mod hooks;
mod job;
mod parser;
mod prompt;
mod shell;
mod signal;
mod structured;
mod suggest;
mod zjump;

use crossterm::terminal;

fn main() {
    // Fast path: non-interactive modes (-c, script) skip Editor/History entirely
    if let Some(code) = shell::run_noninteractive() {
        std::process::exit(code);
    }

    // Interactive mode: set up panic hook to restore terminal
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        default_hook(info);
    }));

    let mut shell = shell::Shell::new();
    shell.run();
}
