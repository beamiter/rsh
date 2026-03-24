/// History management: file I/O, in-memory ring, search.

use std::fs;
use std::io::{BufRead, Write};
use std::path::PathBuf;

pub struct History {
    entries: Vec<String>,
    max_size: usize,
    file_path: PathBuf,
    position: usize, // current navigation position
}

impl History {
    pub fn new(max_size: usize) -> Self {
        let file_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".rsh_history");

        let mut h = History {
            entries: Vec::new(),
            max_size,
            file_path,
            position: 0,
        };
        h.load();
        h
    }

    fn load(&mut self) {
        if let Ok(file) = fs::File::open(&self.file_path) {
            let reader = std::io::BufReader::new(file);
            for line in reader.lines() {
                if let Ok(line) = line {
                    if !line.is_empty() {
                        self.entries.push(line);
                    }
                }
            }
            // Keep only the last max_size entries
            if self.entries.len() > self.max_size {
                let start = self.entries.len() - self.max_size;
                self.entries = self.entries[start..].to_vec();
            }
        }
        self.position = self.entries.len();
    }

    pub fn save(&self) {
        if let Ok(mut file) = fs::File::create(&self.file_path) {
            for entry in &self.entries {
                writeln!(file, "{}", entry).ok();
            }
        }
    }

    pub fn add(&mut self, entry: &str) {
        let entry = entry.trim().to_string();
        if entry.is_empty() { return; }
        // Don't add duplicates of the last entry
        if self.entries.last().map(|s| s.as_str()) == Some(&entry) { return; }
        self.entries.push(entry);
        if self.entries.len() > self.max_size {
            self.entries.remove(0);
        }
        self.position = self.entries.len();
        self.save();
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Navigate to previous entry. Returns the entry or None.
    pub fn prev(&mut self) -> Option<&str> {
        if self.position > 0 {
            self.position -= 1;
            Some(&self.entries[self.position])
        } else {
            None
        }
    }

    /// Navigate to next entry. Returns the entry or None (past end = empty).
    pub fn next(&mut self) -> Option<&str> {
        if self.position < self.entries.len() - 1 {
            self.position += 1;
            Some(&self.entries[self.position])
        } else {
            self.position = self.entries.len();
            None
        }
    }

    /// Reset position to end.
    pub fn reset_position(&mut self) {
        self.position = self.entries.len();
    }

    /// Search backwards from current position for a prefix match.
    pub fn search_prefix(&self, prefix: &str) -> Option<&str> {
        if prefix.is_empty() { return None; }
        for entry in self.entries.iter().rev() {
            if entry.starts_with(prefix) && entry.len() > prefix.len() {
                return Some(entry);
            }
        }
        None
    }

    /// Reverse incremental search.
    pub fn search_substring(&self, query: &str) -> Vec<&str> {
        if query.is_empty() { return Vec::new(); }
        self.entries.iter().rev()
            .filter(|e| e.contains(query))
            .map(|s| s.as_str())
            .collect()
    }
}
