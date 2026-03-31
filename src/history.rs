/// History management: file I/O, in-memory ring, search.

use std::fs::{self, OpenOptions};
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
        self.entries.push(entry.clone());
        if self.entries.len() > self.max_size {
            self.entries.remove(0);
        }
        self.position = self.entries.len();
        // Append just the new line instead of rewriting the entire file
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.file_path) {
            writeln!(file, "{}", entry).ok();
        }
    }

    pub fn last(&self) -> Option<&str> {
        self.entries.last().map(|s| s.as_str())
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(|s| s.as_str())
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
        if !self.entries.is_empty() && self.position + 1 < self.entries.len() {
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

    /// Fuzzy search: find entries where all query chars appear in order.
    /// Returns (entry, matched_indices, score) sorted by score descending.
    pub fn search_fuzzy(&self, query: &str) -> Vec<(String, Vec<usize>)> {
        if query.is_empty() { return Vec::new(); }
        let query_lower: Vec<char> = query.to_lowercase().chars().collect();
        let mut results: Vec<(String, Vec<usize>, i32)> = Vec::new();

        for entry in self.entries.iter().rev() {
            let entry_lower: Vec<char> = entry.to_lowercase().chars().collect();
            if let Some((indices, score)) = fuzzy_match_score(&query_lower, &entry_lower) {
                // Dedup: skip if we already have this exact entry
                if !results.iter().any(|(e, _, _)| e == entry) {
                    results.push((entry.clone(), indices, score));
                }
            }
            if results.len() >= 20 { break; }
        }

        results.sort_by(|a, b| b.2.cmp(&a.2));
        results.into_iter().map(|(e, idx, _)| (e, idx)).collect()
    }
}

fn fuzzy_match_score(query: &[char], candidate: &[char]) -> Option<(Vec<usize>, i32)> {
    let mut qi = 0;
    let mut indices = Vec::new();
    let mut score: i32 = 0;
    let mut prev_match_idx: Option<usize> = None;

    for (ci, &cc) in candidate.iter().enumerate() {
        if qi < query.len() && cc == query[qi] {
            indices.push(ci);
            // Consecutive match bonus
            if let Some(prev) = prev_match_idx {
                if ci == prev + 1 {
                    score += 5;
                }
            }
            // Word boundary bonus
            if ci == 0 || !candidate[ci - 1].is_alphanumeric() {
                score += 3;
            }
            // Prefix bonus
            if ci == qi {
                score += 2;
            }
            prev_match_idx = Some(ci);
            qi += 1;
        }
    }

    if qi == query.len() {
        // Shorter candidates score higher (more relevant)
        score += (100i32).saturating_sub(candidate.len() as i32);
        Some((indices, score))
    } else {
        None
    }
}
