/// Line editor: raw mode, cursor movement, inline editing, integration with
/// highlighting, suggestions, and completion. Supports multiline editing.

use crate::completer::{self, Completion, common_prefix};
use crate::environment::ShellState;
use crate::highlighter;
use crate::history::History;
use crate::prompt;
use crate::signal::SIGINT_RECEIVED;
use crate::suggest;

use crossterm::{
    cursor::{self, MoveToColumn, MoveUp},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    style::{Color, Print, ResetColor, SetAttribute, SetForegroundColor, Attribute},
    terminal::{self, Clear, ClearType},
    ExecutableCommand, QueueableCommand,
};
use std::io::{self, Write, stdout};
use std::sync::atomic::Ordering;
use std::time::Duration;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, PartialEq)]
enum ViMode {
    Normal,
    Insert,
}

pub struct Editor {
    buffer: String,
    cursor: usize,    // byte position in buffer
    saved_buffer: String,
    suggestion: Option<String>,
    terminal_width: u16,
    terminal_height: u16,
    completion_menu: Option<CompletionMenu>,
    search_mode: Option<SearchMode>,
    last_rendered_lines: u16,
    last_cursor_row: u16,
    vi_mode: ViMode,
    vi_pending: Option<char>,
}

struct CompletionMenu {
    completions: Vec<Completion>,
    selected: usize,
    word_start: usize,
    original_word: String,
}

struct SearchMode {
    query: String,
    results: Vec<(String, Vec<usize>)>,
    selected: usize,
}

impl Editor {
    pub fn new() -> Self {
        let (w, h) = terminal::size().unwrap_or((80, 24));
        Editor {
            buffer: String::new(),
            cursor: 0,
            saved_buffer: String::new(),
            suggestion: None,
            terminal_width: w,
            terminal_height: h,
            completion_menu: None,
            search_mode: None,
            last_rendered_lines: 0,
            last_cursor_row: 0,
            vi_mode: ViMode::Insert,
            vi_pending: None,
        }
    }

    pub fn read_line(&mut self, state: &mut ShellState, history: &mut History) -> io::Result<Option<String>> {
        self.buffer.clear();
        self.cursor = 0;
        self.suggestion = None;
        self.saved_buffer.clear();
        self.completion_menu = None;
        self.search_mode = None;
        self.vi_mode = ViMode::Insert;
        self.vi_pending = None;
        history.reset_position();

        let prompt_str = prompt::render_prompt(state);
        let prompt_lines = prompt_str.matches('\n').count() as u16;
        self.last_rendered_lines = prompt_lines;
        self.last_cursor_row = prompt_lines;
        print!("{}", prompt_str);
        io::stdout().flush()?;

        terminal::enable_raw_mode()?;
        stdout().execute(event::EnableBracketedPaste).ok();
        let result = self.edit_loop(state, history);
        stdout().execute(event::DisableBracketedPaste).ok();
        terminal::disable_raw_mode()?;

        result
    }

    fn edit_loop(&mut self, state: &mut ShellState, history: &mut History) -> io::Result<Option<String>> {
        loop {
            if SIGINT_RECEIVED.swap(false, Ordering::SeqCst) {
                self.buffer.clear();
                self.cursor = 0;
                print!("^C\r\n");
                return Ok(Some(String::new()));
            }

            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.code != KeyCode::Tab && key.code != KeyCode::BackTab {
                            if key.code != KeyCode::Enter {
                                if let Some(menu) = self.completion_menu.take() {
                                    if key.code == KeyCode::Esc {
                                        self.buffer.replace_range(menu.word_start..self.cursor, &menu.original_word);
                                        self.cursor = menu.word_start + menu.original_word.len();
                                    }
                                }
                            }
                        }

                        match self.handle_key(key, state, history)? {
                            KeyAction::Continue => {}
                            KeyAction::Submit => {
                                print!("\r\n");
                                let line = self.buffer.clone();
                                return Ok(Some(line));
                            }
                            KeyAction::Eof => {
                                if self.buffer.is_empty() {
                                    print!("\r\n");
                                    return Ok(None);
                                } else {
                                    self.delete_char();
                                }
                            }
                            KeyAction::Interrupt => {
                                print!("^C\r\n");
                                return Ok(Some(String::new()));
                            }
                        }

                        self.update_suggestion(history);
                        self.repaint(state)?;
                    }
                    Event::Paste(text) => {
                        self.buffer.insert_str(self.cursor, &text);
                        self.cursor += text.len();
                        self.update_suggestion(history);
                        self.repaint(state)?;
                    }
                    Event::Resize(w, h) => {
                        self.terminal_width = w;
                        self.terminal_height = h;
                        self.repaint(state)?;
                    }
                    _ => {}
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut ShellState, history: &mut History) -> io::Result<KeyAction> {
        if self.search_mode.is_some() {
            return self.handle_search_key(key, history);
        }

        match state.editing_mode {
            crate::environment::EditingMode::Vi => self.handle_vi_key(key, state, history),
            crate::environment::EditingMode::Emacs => self.handle_emacs_key(key, state, history),
        }
    }

    fn handle_emacs_key(&mut self, key: KeyEvent, state: &mut ShellState, history: &mut History) -> io::Result<KeyAction> {
        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                // Accept completion if menu is open
                if let Some(menu) = self.completion_menu.take() {
                    let completion = &menu.completions[menu.selected];
                    let text = completion.text.clone();
                    let is_dir = completion.is_dir;
                    self.buffer.replace_range(menu.word_start..self.cursor, &text);
                    self.cursor = menu.word_start + text.len();
                    if !is_dir {
                        self.buffer.insert(self.cursor, ' ');
                        self.cursor += 1;
                    }
                    return Ok(KeyAction::Continue);
                }
                // Check if input is incomplete (multiline)
                if crate::parser::is_incomplete(&self.buffer) {
                    self.buffer.push('\n');
                    self.cursor = self.buffer.len();
                    // Auto-indent based on nesting depth
                    let indent = compute_indent(&self.buffer);
                    let indent_str = "    ".repeat(indent);
                    self.buffer.push_str(&indent_str);
                    self.cursor = self.buffer.len();
                    return Ok(KeyAction::Continue);
                }
                return Ok(KeyAction::Submit);
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                return Ok(KeyAction::Eof);
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                return Ok(KeyAction::Interrupt);
            }
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                let mut out = stdout();
                out.execute(Clear(ClearType::All))?;
                out.execute(cursor::MoveTo(0, 0))?;
                self.last_rendered_lines = 0;
                self.last_cursor_row = 0;
            }
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.cursor = self.last_line_start();
            }
            (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                // End of current line (not buffer)
                self.cursor = self.current_line_end();
            }
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                let end = self.current_line_end();
                self.buffer.drain(self.cursor..end);
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                let start = self.last_line_start();
                self.buffer.drain(start..self.cursor);
                self.cursor = start;
            }
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                let new_pos = self.prev_word_boundary();
                self.buffer.drain(new_pos..self.cursor);
                self.cursor = new_pos;
            }
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                self.search_mode = Some(SearchMode {
                    query: String::new(),
                    results: Vec::new(),
                    selected: 0,
                });
            }
            (KeyCode::Tab, _) => {
                self.handle_tab(state);
            }
            (KeyCode::BackTab, _) => {
                if let Some(ref mut menu) = self.completion_menu {
                    if menu.selected == 0 {
                        menu.selected = menu.completions.len() - 1;
                    } else {
                        menu.selected -= 1;
                    }
                    let text = menu.completions[menu.selected].text.clone();
                    self.buffer.replace_range(menu.word_start..self.cursor, &text);
                    self.cursor = menu.word_start + text.len();
                }
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                if self.cursor >= self.buffer.len() {
                    if let Some(suggestion) = self.suggestion.take() {
                        self.buffer.push_str(&suggestion);
                        self.cursor = self.buffer.len();
                    }
                } else {
                    self.move_right();
                }
            }
            (KeyCode::Left, KeyModifiers::NONE) => {
                self.move_left();
            }
            (KeyCode::Home, _) => {
                self.cursor = self.last_line_start();
            }
            (KeyCode::End, _) => {
                self.cursor = self.current_line_end();
            }
            (KeyCode::Up, _) => {
                // Multiline: move cursor up within buffer if not on first line
                let before_cursor = &self.buffer[..self.cursor];
                if before_cursor.contains('\n') {
                    self.move_cursor_up();
                } else {
                    // First line — navigate history
                    if self.cursor == self.buffer.len() && self.saved_buffer.is_empty() {
                        self.saved_buffer = self.buffer.clone();
                    }
                    if let Some(entry) = history.prev() {
                        self.buffer = entry.to_string();
                        self.cursor = self.buffer.len();
                    }
                }
            }
            (KeyCode::Down, _) => {
                // Multiline: move cursor down within buffer if not on last line
                let after_cursor = &self.buffer[self.cursor..];
                if after_cursor.contains('\n') {
                    self.move_cursor_down();
                } else {
                    match history.next() {
                        Some(entry) => {
                            self.buffer = entry.to_string();
                            self.cursor = self.buffer.len();
                        }
                        None => {
                            self.buffer = std::mem::take(&mut self.saved_buffer);
                            self.cursor = self.buffer.len();
                        }
                    }
                }
            }
            (KeyCode::Backspace, _) => {
                if self.cursor > 0 {
                    let prev = self.prev_char_boundary();
                    self.buffer.drain(prev..self.cursor);
                    self.cursor = prev;
                }
            }
            (KeyCode::Delete, _) => {
                self.delete_char();
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                self.buffer.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            _ => {}
        }

        Ok(KeyAction::Continue)
    }

    fn handle_vi_key(&mut self, key: KeyEvent, state: &mut ShellState, history: &mut History) -> io::Result<KeyAction> {
        match self.vi_mode {
            ViMode::Insert => self.handle_vi_insert_key(key, state, history),
            ViMode::Normal => self.handle_vi_normal_key(key, state, history),
        }
    }

    fn handle_vi_insert_key(&mut self, key: KeyEvent, state: &mut ShellState, history: &mut History) -> io::Result<KeyAction> {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.vi_mode = ViMode::Normal;
                // Move cursor back one if possible (vi behavior)
                if self.cursor > 0 {
                    self.move_left();
                }
                // Set block cursor
                print!("\x1b[1 q");
            }
            // In insert mode, most keys behave like Emacs mode
            _ => return self.handle_emacs_key(key, state, history),
        }
        Ok(KeyAction::Continue)
    }

    fn handle_vi_normal_key(&mut self, key: KeyEvent, _state: &mut ShellState, history: &mut History) -> io::Result<KeyAction> {
        // Handle pending multi-char commands (dd, dw, etc.)
        if let Some(pending) = self.vi_pending.take() {
            return self.handle_vi_pending(pending, key);
        }

        match (key.code, key.modifiers) {
            // Mode switching
            (KeyCode::Char('i'), KeyModifiers::NONE) => {
                self.vi_mode = ViMode::Insert;
                print!("\x1b[5 q"); // line cursor
            }
            (KeyCode::Char('a'), KeyModifiers::NONE) => {
                self.vi_mode = ViMode::Insert;
                self.move_right();
                print!("\x1b[5 q");
            }
            (KeyCode::Char('A'), KeyModifiers::SHIFT) => {
                self.vi_mode = ViMode::Insert;
                self.cursor = self.current_line_end();
                print!("\x1b[5 q");
            }
            (KeyCode::Char('I'), KeyModifiers::SHIFT) => {
                self.vi_mode = ViMode::Insert;
                self.cursor = self.last_line_start();
                print!("\x1b[5 q");
            }
            (KeyCode::Char('o'), KeyModifiers::NONE) => {
                self.vi_mode = ViMode::Insert;
                self.cursor = self.current_line_end();
                self.buffer.insert(self.cursor, '\n');
                self.cursor += 1;
                print!("\x1b[5 q");
            }
            (KeyCode::Char('O'), KeyModifiers::SHIFT) => {
                self.vi_mode = ViMode::Insert;
                let start = self.last_line_start();
                self.buffer.insert(start, '\n');
                self.cursor = start;
                print!("\x1b[5 q");
            }

            // Movement
            (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, _) => {
                self.move_left();
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, _) => {
                if self.cursor < self.buffer.len() {
                    self.move_right();
                }
            }
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
                let after_cursor = &self.buffer[self.cursor..];
                if after_cursor.contains('\n') {
                    self.move_cursor_down();
                } else {
                    match history.next() {
                        Some(entry) => {
                            self.buffer = entry.to_string();
                            self.cursor = self.buffer.len();
                        }
                        None => {
                            self.buffer = std::mem::take(&mut self.saved_buffer);
                            self.cursor = self.buffer.len();
                        }
                    }
                }
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
                let before_cursor = &self.buffer[..self.cursor];
                if before_cursor.contains('\n') {
                    self.move_cursor_up();
                } else {
                    if self.cursor == self.buffer.len() && self.saved_buffer.is_empty() {
                        self.saved_buffer = self.buffer.clone();
                    }
                    if let Some(entry) = history.prev() {
                        self.buffer = entry.to_string();
                        self.cursor = self.buffer.len();
                    }
                }
            }
            (KeyCode::Char('0'), KeyModifiers::NONE) => {
                self.cursor = self.last_line_start();
            }
            (KeyCode::Char('$'), KeyModifiers::SHIFT) | (KeyCode::End, _) => {
                self.cursor = self.current_line_end();
                // In normal mode, cursor sits ON the last char, not past it
                let end = self.current_line_end();
                if end > self.last_line_start() {
                    self.cursor = self.prev_char_boundary_from(end);
                }
            }
            (KeyCode::Char('^'), KeyModifiers::SHIFT) | (KeyCode::Home, _) => {
                // Go to first non-whitespace char
                let start = self.last_line_start();
                let end = self.current_line_end();
                let line = &self.buffer[start..end];
                let indent = line.len() - line.trim_start().len();
                self.cursor = start + indent;
            }

            // Word movement
            (KeyCode::Char('w'), KeyModifiers::NONE) => {
                self.vi_word_forward();
            }
            (KeyCode::Char('b'), KeyModifiers::NONE) => {
                self.vi_word_backward();
            }
            (KeyCode::Char('e'), KeyModifiers::NONE) => {
                self.vi_word_end();
            }

            // Editing
            (KeyCode::Char('x'), KeyModifiers::NONE) => {
                self.delete_char();
            }
            (KeyCode::Char('X'), KeyModifiers::SHIFT) => {
                if self.cursor > 0 {
                    let prev = self.prev_char_boundary();
                    self.buffer.drain(prev..self.cursor);
                    self.cursor = prev;
                }
            }
            (KeyCode::Char('d'), KeyModifiers::NONE) => {
                self.vi_pending = Some('d');
            }
            (KeyCode::Char('c'), KeyModifiers::NONE) => {
                self.vi_pending = Some('c');
            }
            (KeyCode::Char('C'), KeyModifiers::SHIFT) => {
                // Change to end of line
                let end = self.current_line_end();
                self.buffer.drain(self.cursor..end);
                self.vi_mode = ViMode::Insert;
                print!("\x1b[5 q");
            }
            (KeyCode::Char('D'), KeyModifiers::SHIFT) => {
                // Delete to end of line
                let end = self.current_line_end();
                self.buffer.drain(self.cursor..end);
            }
            (KeyCode::Char('s'), KeyModifiers::NONE) => {
                // Substitute char
                self.delete_char();
                self.vi_mode = ViMode::Insert;
                print!("\x1b[5 q");
            }
            (KeyCode::Char('S'), KeyModifiers::SHIFT) => {
                // Substitute line
                let start = self.last_line_start();
                let end = self.current_line_end();
                self.buffer.drain(start..end);
                self.cursor = start;
                self.vi_mode = ViMode::Insert;
                print!("\x1b[5 q");
            }
            (KeyCode::Char('r'), KeyModifiers::NONE) => {
                self.vi_pending = Some('r');
            }
            (KeyCode::Char('p'), KeyModifiers::NONE) => {
                // Paste - not implemented (no clipboard)
            }

            // Search
            (KeyCode::Char('/'), KeyModifiers::NONE) => {
                self.search_mode = Some(SearchMode {
                    query: String::new(),
                    results: Vec::new(),
                    selected: 0,
                });
            }

            // Enter submits in normal mode
            (KeyCode::Enter, _) => {
                // Accept completion if menu open
                if let Some(menu) = self.completion_menu.take() {
                    let completion = &menu.completions[menu.selected];
                    let text = completion.text.clone();
                    let is_dir = completion.is_dir;
                    self.buffer.replace_range(menu.word_start..self.cursor, &text);
                    self.cursor = menu.word_start + text.len();
                    if !is_dir {
                        self.buffer.insert(self.cursor, ' ');
                        self.cursor += 1;
                    }
                    return Ok(KeyAction::Continue);
                }
                if crate::parser::is_incomplete(&self.buffer) {
                    self.buffer.push('\n');
                    self.cursor = self.buffer.len();
                    let indent = compute_indent(&self.buffer);
                    let indent_str = "    ".repeat(indent);
                    self.buffer.push_str(&indent_str);
                    self.cursor = self.buffer.len();
                    return Ok(KeyAction::Continue);
                }
                return Ok(KeyAction::Submit);
            }

            // Ctrl+C interrupt
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                return Ok(KeyAction::Interrupt);
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                return Ok(KeyAction::Eof);
            }
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                let mut out = stdout();
                out.execute(Clear(ClearType::All))?;
                out.execute(cursor::MoveTo(0, 0))?;
                self.last_rendered_lines = 0;
                self.last_cursor_row = 0;
            }

            _ => {}
        }
        Ok(KeyAction::Continue)
    }

    fn handle_vi_pending(&mut self, pending: char, key: KeyEvent) -> io::Result<KeyAction> {
        match pending {
            'd' => {
                match key.code {
                    KeyCode::Char('d') => {
                        // dd: delete entire line
                        let start = self.last_line_start();
                        let end = self.current_line_end();
                        // Also delete the newline if there is one
                        if end < self.buffer.len() && self.buffer.as_bytes()[end] == b'\n' {
                            self.buffer.drain(start..end + 1);
                            self.cursor = start.min(self.buffer.len().saturating_sub(1));
                        } else if start > 0 {
                            // Delete preceding newline instead
                            let new_start = start - 1;
                            self.buffer.drain(new_start..end);
                            self.cursor = new_start.min(self.buffer.len());
                        } else {
                            self.buffer.drain(start..end);
                            self.cursor = start.min(self.buffer.len().saturating_sub(1));
                        }
                    }
                    KeyCode::Char('w') => {
                        // dw: delete word
                        let start = self.cursor;
                        self.vi_word_forward();
                        let end = self.cursor;
                        self.buffer.drain(start..end);
                        self.cursor = start;
                    }
                    KeyCode::Char('$') => {
                        // d$: delete to end of line
                        let end = self.current_line_end();
                        self.buffer.drain(self.cursor..end);
                    }
                    KeyCode::Char('0') => {
                        // d0: delete to start of line
                        let start = self.last_line_start();
                        self.buffer.drain(start..self.cursor);
                        self.cursor = start;
                    }
                    _ => {}
                }
            }
            'c' => {
                match key.code {
                    KeyCode::Char('c') => {
                        // cc: change entire line
                        let start = self.last_line_start();
                        let end = self.current_line_end();
                        self.buffer.drain(start..end);
                        self.cursor = start;
                        self.vi_mode = ViMode::Insert;
                        print!("\x1b[5 q");
                    }
                    KeyCode::Char('w') => {
                        // cw: change word
                        let start = self.cursor;
                        self.vi_word_forward();
                        let end = self.cursor;
                        self.buffer.drain(start..end);
                        self.cursor = start;
                        self.vi_mode = ViMode::Insert;
                        print!("\x1b[5 q");
                    }
                    _ => {}
                }
            }
            'r' => {
                // Replace single character
                if let KeyCode::Char(c) = key.code {
                    if self.cursor < self.buffer.len() {
                        let old_char = self.buffer[self.cursor..].chars().next().unwrap();
                        self.buffer.replace_range(self.cursor..self.cursor + old_char.len_utf8(), &c.to_string());
                    }
                }
            }
            _ => {}
        }
        Ok(KeyAction::Continue)
    }

    fn handle_search_key(&mut self, key: KeyEvent, history: &mut History) -> io::Result<KeyAction> {
        let search = self.search_mode.as_mut().unwrap();
        match key.code {
            KeyCode::Esc => {
                self.search_mode = None;
            }
            KeyCode::Enter => {
                if let Some((result, _)) = search.results.get(search.selected) {
                    self.buffer = result.clone();
                    self.cursor = self.buffer.len();
                }
                self.search_mode = None;
            }
            KeyCode::Char('r') if key.modifiers == KeyModifiers::CONTROL => {
                if !search.results.is_empty() {
                    search.selected = (search.selected + 1) % search.results.len();
                    if let Some((result, _)) = search.results.get(search.selected) {
                        self.buffer = result.clone();
                        self.cursor = self.buffer.len();
                    }
                }
            }
            KeyCode::Backspace => {
                search.query.pop();
                search.results = history.search_fuzzy(&search.query);
                search.selected = 0;
                if let Some((result, _)) = search.results.first() {
                    self.buffer = result.clone();
                    self.cursor = self.buffer.len();
                }
            }
            KeyCode::Char(c) if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT => {
                search.query.push(c);
                search.results = history.search_fuzzy(&search.query);
                search.selected = 0;
                if let Some((result, _)) = search.results.first() {
                    self.buffer = result.clone();
                    self.cursor = self.buffer.len();
                }
            }
            _ => {
                self.search_mode = None;
            }
        }
        Ok(KeyAction::Continue)
    }

    fn handle_tab(&mut self, state: &mut ShellState) {
        if let Some(ref mut menu) = self.completion_menu {
            menu.selected = (menu.selected + 1) % menu.completions.len();
            let text = menu.completions[menu.selected].text.clone();
            self.buffer.replace_range(menu.word_start..self.cursor, &text);
            self.cursor = menu.word_start + text.len();
            return;
        }

        let (word_start, completions) = completer::complete(&self.buffer, self.cursor, state);

        match completions.len() {
            0 => {}
            1 => {
                let text = &completions[0].text;
                self.buffer.replace_range(word_start..self.cursor, text);
                self.cursor = word_start + text.len();
                if !completions[0].is_dir {
                    self.buffer.insert(self.cursor, ' ');
                    self.cursor += 1;
                }
            }
            _ => {
                let common = common_prefix(&completions);
                if common.len() > self.cursor - word_start {
                    self.buffer.replace_range(word_start..self.cursor, &common);
                    self.cursor = word_start + common.len();
                } else {
                    let original_word = self.buffer[word_start..self.cursor].to_string();
                    self.completion_menu = Some(CompletionMenu {
                        completions,
                        selected: 0,
                        word_start,
                        original_word,
                    });
                }
            }
        }
    }

    fn update_suggestion(&mut self, history: &History) {
        if self.completion_menu.is_some() || self.search_mode.is_some() {
            self.suggestion = None;
            return;
        }
        self.suggestion = suggest::suggest(&self.buffer, history);
    }

    fn repaint(&mut self, state: &mut ShellState) -> io::Result<()> {
        let mut out = stdout();

        if state.editing_mode == crate::environment::EditingMode::Vi {
            match self.vi_mode {
                ViMode::Normal => { out.queue(Print("\x1b[1 q"))?; } // block cursor
                ViMode::Insert => { out.queue(Print("\x1b[5 q"))?; } // line cursor
            }
        }

        out.queue(MoveToColumn(0))?;
        if self.last_cursor_row > 0 {
            out.queue(MoveUp(self.last_cursor_row))?;
        }
        out.queue(Clear(ClearType::FromCursorDown))?;

        let mut rendered_lines: u16 = 0;
        #[allow(unused_assignments)]
        let mut cursor_row: u16 = 0;
        #[allow(unused_assignments)]
        let mut cursor_col: u16 = 0;

        if let Some(ref search) = self.search_mode {
            let count = search.results.len();
            let sel = if count > 0 { search.selected + 1 } else { 0 };
            out.queue(SetForegroundColor(Color::Yellow))?;
            out.queue(Print(format!("(search [{}/{}])`{}': ", sel, count, search.query)))?;
            out.queue(ResetColor)?;
            out.queue(Print(&self.buffer))?;
            let prefix = format!("(search [{}/{}])`{}': ", sel, count, search.query);
            cursor_col = (prefix.len() + self.buffer.len()) as u16;
            cursor_row = 0;
        } else {
            // Render prompt
            let prompt_text = prompt::render_prompt(state);
            let prompt_lines = prompt_text.matches('\n').count() as u16;
            rendered_lines += prompt_lines;
            out.queue(Print(&prompt_text))?;

            // Render highlighted buffer with continuation prompts
            let spans = highlighter::highlight(&self.buffer, state);
            let cont_prompt = prompt::render_continuation_prompt();
            for span in &spans {
                if let Some(color) = span.fg {
                    out.queue(SetForegroundColor(color))?;
                }
                if span.bold {
                    out.queue(SetAttribute(Attribute::Bold))?;
                }
                if span.underline {
                    out.queue(SetAttribute(Attribute::Underlined))?;
                }
                // Handle newlines within spans — insert continuation prompt
                let lines: Vec<&str> = span.text.split('\n').collect();
                for (li, line) in lines.iter().enumerate() {
                    out.queue(Print(line))?;
                    if li < lines.len() - 1 {
                        out.queue(ResetColor)?;
                        out.queue(SetAttribute(Attribute::Reset))?;
                        out.queue(Print("\r\n"))?;
                        out.queue(Print(&cont_prompt))?;
                        rendered_lines += 1;
                        // Re-apply colors for next segment
                        if let Some(color) = span.fg {
                            out.queue(SetForegroundColor(color))?;
                        }
                        if span.bold {
                            out.queue(SetAttribute(Attribute::Bold))?;
                        }
                    }
                }
                out.queue(ResetColor)?;
                out.queue(SetAttribute(Attribute::Reset))?;
            }

            // Render right prompt on first line if there's room
            let rprompt = prompt::render_rprompt(state);
            let rprompt_w = prompt::rprompt_width(state);
            if rprompt_w > 0 {
                let first_line = self.buffer.split('\n').next().unwrap_or("");
                let prompt_last = prompt_text.rsplit('\n').next().unwrap_or(&prompt_text);
                let content_width = display_width(prompt_last) + display_width(first_line);
                let available = self.terminal_width as usize;
                if content_width + rprompt_w + 2 < available {
                    out.queue(cursor::SavePosition)?;
                    out.queue(MoveToColumn((available - rprompt_w) as u16))?;
                    out.queue(Print(&rprompt))?;
                    out.queue(cursor::RestorePosition)?;
                }
            }

            // Render suggestion (ghost text)
            if let Some(ref suggestion) = self.suggestion {
                if self.cursor == self.buffer.len() {
                    out.queue(SetForegroundColor(Color::DarkGrey))?;
                    out.queue(Print(suggestion))?;
                    out.queue(ResetColor)?;
                }
            }

            // Compute cursor screen position accounting for continuation prompts
            let prompt_last_line = prompt_text.rsplit('\n').next().unwrap_or(&prompt_text);
            let prompt_width = display_width(prompt_last_line) as u16;
            let cont_width = display_width(&cont_prompt) as u16;
            let buf_before = &self.buffer[..self.cursor];
            let buf_cursor_lines = buf_before.matches('\n').count() as u16;
            let buf_cursor_last = buf_before.rsplit('\n').next().unwrap_or(buf_before);
            cursor_row = prompt_lines + buf_cursor_lines;
            cursor_col = if buf_cursor_lines > 0 {
                cont_width + display_width(buf_cursor_last) as u16
            } else {
                prompt_width + display_width(buf_cursor_last) as u16
            };
        }

        // Render completion menu if active
        if let Some(ref menu) = self.completion_menu {
            out.queue(Print("\r\n"))?;
            rendered_lines += 1;
            let cols = (self.terminal_width as usize) / 20;
            let cols = cols.max(1);
            for (i, comp) in menu.completions.iter().enumerate().take(20) {
                if i == menu.selected {
                    out.queue(SetForegroundColor(Color::Black))?;
                    out.queue(SetAttribute(Attribute::Reverse))?;
                }
                let display = if let Some(ref desc) = comp.description {
                    format!("{:<16} {}", comp.display, desc)
                } else {
                    format!("{:<18}", comp.display)
                };
                out.queue(Print(&display))?;
                if i == menu.selected {
                    out.queue(ResetColor)?;
                    out.queue(SetAttribute(Attribute::Reset))?;
                }
                out.queue(Print("  "))?;
                if (i + 1) % cols == 0 && i + 1 < menu.completions.len() {
                    out.queue(Print("\r\n"))?;
                    rendered_lines += 1;
                }
            }
            if menu.completions.len() > 20 {
                out.queue(Print(format!("\r\n... and {} more", menu.completions.len() - 20)))?;
                rendered_lines += 1;
            }
        }

        let go_up = rendered_lines.saturating_sub(cursor_row);
        if go_up > 0 {
            out.queue(MoveUp(go_up))?;
        }
        out.queue(MoveToColumn(cursor_col))?;

        self.last_rendered_lines = rendered_lines;
        self.last_cursor_row = cursor_row;
        out.flush()?;
        Ok(())
    }

    // Character/cursor helpers

    fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            let c = self.buffer[self.cursor..].chars().next().unwrap();
            self.cursor += c.len_utf8();
        }
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.prev_char_boundary();
        }
    }

    fn prev_char_boundary(&self) -> usize {
        let mut pos = self.cursor - 1;
        while pos > 0 && !self.buffer.is_char_boundary(pos) {
            pos -= 1;
        }
        pos
    }

    fn last_line_start(&self) -> usize {
        match self.buffer[..self.cursor].rfind('\n') {
            Some(pos) => pos + 1,
            None => 0,
        }
    }

    fn current_line_end(&self) -> usize {
        match self.buffer[self.cursor..].find('\n') {
            Some(pos) => self.cursor + pos,
            None => self.buffer.len(),
        }
    }

    fn prev_word_boundary(&self) -> usize {
        let buf = &self.buffer[..self.cursor];
        let trimmed = buf.trim_end();
        match trimmed.rfind(|c: char| c == ' ' || c == '\t' || c == '/') {
            Some(pos) => pos + 1,
            None => 0,
        }
    }

    fn delete_char(&mut self) {
        if self.cursor < self.buffer.len() {
            let c = self.buffer[self.cursor..].chars().next().unwrap();
            self.buffer.drain(self.cursor..self.cursor + c.len_utf8());
        }
    }

    // Multiline cursor movement

    fn current_line_col(&self) -> (usize, usize) {
        let before = &self.buffer[..self.cursor];
        match before.rfind('\n') {
            Some(nl) => (nl + 1, display_width_raw(&before[nl + 1..])),
            None => (0, display_width_raw(before)),
        }
    }

    fn move_cursor_up(&mut self) {
        let (line_start, col) = self.current_line_col();
        if line_start == 0 { return; }
        let prev_nl = self.buffer[..line_start - 1].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let prev_line = &self.buffer[prev_nl..line_start - 1];
        let target_col = col.min(display_width_raw(prev_line));
        // Walk chars until we reach target display column
        let mut current_col = 0;
        let mut byte_offset = 0;
        for c in prev_line.chars() {
            if current_col >= target_col { break; }
            current_col += char_width(c);
            byte_offset += c.len_utf8();
        }
        self.cursor = prev_nl + byte_offset;
    }

    fn move_cursor_down(&mut self) {
        let (_line_start, col) = self.current_line_col();
        let next_nl = self.buffer[self.cursor..].find('\n');
        if next_nl.is_none() { return; }
        let next_line_start = self.cursor + next_nl.unwrap() + 1;
        let next_line_end = self.buffer[next_line_start..].find('\n')
            .map(|p| next_line_start + p)
            .unwrap_or(self.buffer.len());
        let next_line = &self.buffer[next_line_start..next_line_end];
        let target_col = col.min(display_width_raw(next_line));
        let mut current_col = 0;
        let mut byte_offset = 0;
        for c in next_line.chars() {
            if current_col >= target_col { break; }
            current_col += char_width(c);
            byte_offset += c.len_utf8();
        }
        self.cursor = next_line_start + byte_offset;
    }

    // Vi word movement helpers

    fn vi_word_forward(&mut self) {
        let buf = &self.buffer[self.cursor..];
        let mut chars = buf.chars();
        let mut moved = 0;
        // Skip current word (non-whitespace)
        for c in chars.by_ref() {
            moved += c.len_utf8();
            if c.is_whitespace() { break; }
        }
        // Skip whitespace
        for c in chars {
            if !c.is_whitespace() { break; }
            moved += c.len_utf8();
        }
        self.cursor = (self.cursor + moved).min(self.buffer.len());
    }

    fn vi_word_backward(&mut self) {
        if self.cursor == 0 { return; }
        let buf = &self.buffer[..self.cursor];
        let mut pos = self.cursor;
        // Skip trailing whitespace
        for c in buf.chars().rev() {
            if !c.is_whitespace() { break; }
            pos -= c.len_utf8();
        }
        // Skip word chars
        let buf = &self.buffer[..pos];
        for c in buf.chars().rev() {
            if c.is_whitespace() { break; }
            pos -= c.len_utf8();
        }
        self.cursor = pos;
    }

    fn vi_word_end(&mut self) {
        if self.cursor >= self.buffer.len() { return; }
        let start = self.cursor + self.buffer[self.cursor..].chars().next().map_or(0, |c| c.len_utf8());
        let buf = &self.buffer[start..];
        let mut moved = 0;
        let mut chars = buf.chars();
        // Skip whitespace
        for c in chars.by_ref() {
            moved += c.len_utf8();
            if !c.is_whitespace() { break; }
        }
        // Move to end of word
        for c in chars {
            if c.is_whitespace() { break; }
            moved += c.len_utf8();
        }
        self.cursor = (start + moved).min(self.buffer.len());
    }

    fn prev_char_boundary_from(&self, pos: usize) -> usize {
        if pos == 0 { return 0; }
        let mut p = pos - 1;
        while p > 0 && !self.buffer.is_char_boundary(p) {
            p -= 1;
        }
        p
    }
}

enum KeyAction {
    Continue,
    Submit,
    Eof,
    Interrupt,
}

/// Compute indentation depth for auto-indent in multiline editing.
fn compute_indent(buffer: &str) -> usize {
    let tokens = crate::parser::lexer::tokenize_lenient(buffer);
    let mut depth: i32 = 0;
    use crate::parser::lexer::Token;
    for t in &tokens {
        match &t.token {
            Token::LBrace | Token::LParen => depth += 1,
            Token::RBrace | Token::RParen => depth -= 1,
            Token::Word(w) => match w.as_str() {
                "do" | "then" => depth += 1,
                "done" | "fi" | "esac" => depth -= 1,
                _ => {}
            },
            _ => {}
        }
    }
    depth.max(0) as usize
}

/// Calculate display width of a string, stripping ANSI escape sequences.
fn display_width(s: &str) -> usize {
    let mut w = 0;
    let mut in_esc = false;
    for c in s.chars() {
        if in_esc {
            if c.is_ascii_alphabetic() {
                in_esc = false;
            }
        } else if c == '\x1b' {
            in_esc = true;
        } else {
            w += char_width(c);
        }
    }
    w
}

fn char_width(c: char) -> usize {
    if c == '\0' { return 0; }
    UnicodeWidthChar::width(c).unwrap_or(0)
}

fn display_width_raw(s: &str) -> usize {
    s.chars().map(char_width).sum()
}
