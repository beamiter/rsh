/// Line editor: raw mode, cursor movement, inline editing, integration with
/// highlighting, suggestions, and completion.

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

pub struct Editor {
    buffer: String,
    cursor: usize,    // byte position in buffer
    saved_buffer: String, // saved buffer during history navigation
    suggestion: Option<String>,
    terminal_width: u16,
    terminal_height: u16,
    completion_menu: Option<CompletionMenu>,
    search_mode: Option<SearchMode>,
    last_rendered_lines: u16,
}

struct CompletionMenu {
    completions: Vec<Completion>,
    selected: usize,
    word_start: usize,
}

struct SearchMode {
    query: String,
    results: Vec<String>,
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
        }
    }

    pub fn read_line(&mut self, state: &mut ShellState, history: &mut History) -> io::Result<Option<String>> {
        self.buffer.clear();
        self.cursor = 0;
        self.suggestion = None;
        self.saved_buffer.clear();
        self.completion_menu = None;
        self.search_mode = None;
        history.reset_position();

        // Print prompt
        let prompt_str = prompt::render_prompt(state);
        self.last_rendered_lines = prompt_str.matches('\n').count() as u16;
        print!("{}", prompt_str);
        io::stdout().flush()?;

        terminal::enable_raw_mode()?;
        let result = self.edit_loop(state, history);
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
                        // Close completion menu on non-tab keys
                        if key.code != KeyCode::Tab && key.code != KeyCode::BackTab {
                            if self.completion_menu.is_some() && key.code != KeyCode::Enter {
                                self.completion_menu = None;
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
                                    return Ok(None); // EOF
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
                    Event::Resize(w, h) => {
                        self.terminal_width = w;
                        self.terminal_height = h;
                        self.repaint(state)?;
                    }
                    _ => {}
                }
            } else {
                // Timeout - check for background job notifications
                // Job notifications handled by shell main loop
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut ShellState, history: &mut History) -> io::Result<KeyAction> {
        // Handle reverse search mode
        if self.search_mode.is_some() {
            return self.handle_search_key(key, history);
        }

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
                // Clear screen
                let mut out = stdout();
                out.execute(Clear(ClearType::All))?;
                out.execute(cursor::MoveTo(0, 0))?;
                self.last_rendered_lines = 0;
            }
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                // Home
                self.cursor = self.last_line_start();
            }
            (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                // End
                self.cursor = self.buffer.len();
            }
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                // Kill to end of line
                self.buffer.truncate(self.cursor);
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                // Kill to start of line
                let start = self.last_line_start();
                self.buffer.drain(start..self.cursor);
                self.cursor = start;
            }
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                // Kill previous word
                let new_pos = self.prev_word_boundary();
                self.buffer.drain(new_pos..self.cursor);
                self.cursor = new_pos;
            }
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                // Enter reverse search mode
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
                // Shift+Tab: cycle backwards in completion menu
                if let Some(ref mut menu) = self.completion_menu {
                    if menu.selected == 0 {
                        menu.selected = menu.completions.len() - 1;
                    } else {
                        menu.selected -= 1;
                    }
                }
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                if self.cursor >= self.buffer.len() {
                    // Accept suggestion
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
                self.cursor = self.buffer.len();
            }
            (KeyCode::Up, _) => {
                // History previous
                if self.cursor == self.buffer.len() && self.saved_buffer.is_empty() {
                    self.saved_buffer = self.buffer.clone();
                }
                if let Some(entry) = history.prev() {
                    self.buffer = entry.to_string();
                    self.cursor = self.buffer.len();
                }
            }
            (KeyCode::Down, _) => {
                // History next
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

    fn handle_search_key(&mut self, key: KeyEvent, history: &mut History) -> io::Result<KeyAction> {
        let search = self.search_mode.as_mut().unwrap();
        match key.code {
            KeyCode::Esc => {
                self.search_mode = None;
            }
            KeyCode::Enter => {
                if let Some(result) = search.results.get(search.selected) {
                    self.buffer = result.clone();
                    self.cursor = self.buffer.len();
                }
                self.search_mode = None;
            }
            KeyCode::Char('r') if key.modifiers == KeyModifiers::CONTROL => {
                // Next result
                if !search.results.is_empty() {
                    search.selected = (search.selected + 1) % search.results.len();
                    if let Some(result) = search.results.get(search.selected) {
                        self.buffer = result.clone();
                        self.cursor = self.buffer.len();
                    }
                }
            }
            KeyCode::Backspace => {
                search.query.pop();
                search.results = history.search_substring(&search.query)
                    .into_iter().map(|s| s.to_string()).collect();
                search.selected = 0;
                if let Some(result) = search.results.first() {
                    self.buffer = result.clone();
                    self.cursor = self.buffer.len();
                }
            }
            KeyCode::Char(c) if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT => {
                search.query.push(c);
                search.results = history.search_substring(&search.query)
                    .into_iter().map(|s| s.to_string()).collect();
                search.selected = 0;
                if let Some(result) = search.results.first() {
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

    fn handle_tab(&mut self, state: &ShellState) {
        if let Some(ref mut menu) = self.completion_menu {
            // Cycle highlight only - Enter to confirm
            menu.selected = (menu.selected + 1) % menu.completions.len();
            return;
        }

        let (word_start, completions) = completer::complete(&self.buffer, self.cursor, state);

        match completions.len() {
            0 => {} // No completions
            1 => {
                // Single completion - insert it
                let text = &completions[0].text;
                self.buffer.replace_range(word_start..self.cursor, text);
                self.cursor = word_start + text.len();
                // Add space after non-directory completions
                if !completions[0].is_dir {
                    self.buffer.insert(self.cursor, ' ');
                    self.cursor += 1;
                }
            }
            _ => {
                // Multiple - insert common prefix first
                let common = common_prefix(&completions);
                if common.len() > self.cursor - word_start {
                    self.buffer.replace_range(word_start..self.cursor, &common);
                    self.cursor = word_start + common.len();
                } else {
                    // Show completion menu
                    self.completion_menu = Some(CompletionMenu {
                        completions,
                        selected: 0,
                        word_start,
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

    fn repaint(&mut self, state: &ShellState) -> io::Result<()> {
        let mut out = stdout();

        // Move back to start of previous render
        out.queue(MoveToColumn(0))?;
        if self.last_rendered_lines > 0 {
            out.queue(MoveUp(self.last_rendered_lines))?;
        }
        out.queue(Clear(ClearType::FromCursorDown))?;

        let mut rendered_lines: u16 = 0;

        if let Some(ref search) = self.search_mode {
            // Render search prompt (single line)
            out.queue(SetForegroundColor(Color::Yellow))?;
            out.queue(Print(format!("(reverse-i-search)`{}': ", search.query)))?;
            out.queue(ResetColor)?;
            out.queue(Print(&self.buffer))?;
        } else {
            // Render prompt
            let prompt_text = prompt::render_prompt(state);
            rendered_lines += prompt_text.matches('\n').count() as u16;
            out.queue(Print(&prompt_text))?;

            // Render highlighted buffer
            let spans = highlighter::highlight(&self.buffer, state);
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
                out.queue(Print(&span.text))?;
                out.queue(ResetColor)?;
                out.queue(SetAttribute(Attribute::Reset))?;
            }
            rendered_lines += self.buffer.matches('\n').count() as u16;

            // Render suggestion (ghost text)
            if let Some(ref suggestion) = self.suggestion {
                if self.cursor == self.buffer.len() {
                    out.queue(SetForegroundColor(Color::DarkGrey))?;
                    out.queue(Print(suggestion))?;
                    out.queue(ResetColor)?;
                }
            }
        }

        // Render completion menu if active
        if let Some(ref menu) = self.completion_menu {
            out.queue(Print("\n"))?;
            rendered_lines += 1;
            let cols = (self.terminal_width as usize) / 20;
            let cols = cols.max(1);
            for (i, comp) in menu.completions.iter().enumerate().take(20) {
                if i == menu.selected {
                    out.queue(SetForegroundColor(Color::Black))?;
                    out.queue(SetAttribute(Attribute::Reverse))?;
                }
                let display = format!("{:<18}", comp.display);
                out.queue(Print(&display))?;
                if i == menu.selected {
                    out.queue(ResetColor)?;
                    out.queue(SetAttribute(Attribute::Reset))?;
                }
                out.queue(Print("  "))?;
                if (i + 1) % cols == 0 && i + 1 < menu.completions.len() {
                    out.queue(Print("\n"))?;
                    rendered_lines += 1;
                }
            }
            if menu.completions.len() > 20 {
                out.queue(Print(format!("\n... and {} more", menu.completions.len() - 20)))?;
                rendered_lines += 1;
            }
        }

        self.last_rendered_lines = rendered_lines;
        out.flush()?;
        Ok(())
    }

    fn get_display_buffer(&self) -> String {
        let mut display = self.buffer.clone();
        if let Some(ref suggestion) = self.suggestion {
            if self.cursor == self.buffer.len() {
                display.push_str(suggestion);
            }
        }
        display
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
}

enum KeyAction {
    Continue,
    Submit,
    Eof,
    Interrupt,
}

