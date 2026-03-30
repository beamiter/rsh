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
}

struct CompletionMenu {
    completions: Vec<Completion>,
    selected: usize,
    word_start: usize,
    original_word: String,
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
            last_cursor_row: 0,
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

        let prompt_str = prompt::render_prompt(state);
        let prompt_lines = prompt_str.matches('\n').count() as u16;
        self.last_rendered_lines = prompt_lines;
        self.last_cursor_row = prompt_lines;
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
            out.queue(SetForegroundColor(Color::Yellow))?;
            out.queue(Print(format!("(reverse-i-search)`{}': ", search.query)))?;
            out.queue(ResetColor)?;
            out.queue(Print(&self.buffer))?;
            let prefix = format!("(reverse-i-search)`{}': ", search.query);
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
            Some(nl) => (nl + 1, before[nl + 1..].chars().count()),
            None => (0, before.chars().count()),
        }
    }

    fn move_cursor_up(&mut self) {
        let (line_start, col) = self.current_line_col();
        if line_start == 0 { return; }
        let prev_nl = self.buffer[..line_start - 1].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let prev_line = &self.buffer[prev_nl..line_start - 1];
        let target_col = col.min(prev_line.chars().count());
        self.cursor = prev_nl + prev_line.chars().take(target_col).map(|c| c.len_utf8()).sum::<usize>();
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
        let target_col = col.min(next_line.chars().count());
        self.cursor = next_line_start + next_line.chars().take(target_col).map(|c| c.len_utf8()).sum::<usize>();
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
    let cp = c as u32;
    if (0x4E00..=0x9FFF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x20000..=0x2A6DF).contains(&cp)
        || (0x2A700..=0x2CEAF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xAC00..=0xD7AF).contains(&cp)
        || (0xFF01..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x2E80..=0x303E).contains(&cp)
        || (0x3041..=0x33BF).contains(&cp)
        || (0xFE30..=0xFE6F).contains(&cp)
    {
        2
    } else {
        1
    }
}
