#![allow(dead_code)]

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
mod job;
mod parser;
mod prompt;
mod shell;
mod signal;
mod suggest;

use crossterm::terminal;

fn main() {
    // Panic hook: restore terminal on crash
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        default_hook(info);
    }));

    let mut shell = shell::Shell::new();
    shell.run();
}
