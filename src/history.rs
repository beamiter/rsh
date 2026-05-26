/// History management: file I/O, in-memory ring, search.
/// Supports timestamped entries for rich Ctrl+R panel display.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct HistoryEntry {
    pub command: String,
    pub timestamp: u64,
    pub cwd: Option<String>,
}

pub struct History {
    entries: Vec<HistoryEntry>,
    max_size: usize,
    file_path: PathBuf,
    position: usize,
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
                    if line.is_empty() { continue; }
                    self.entries.push(Self::parse_line(&line));
                }
            }
            if self.entries.len() > self.max_size {
                let start = self.entries.len() - self.max_size;
                self.entries = self.entries[start..].to_vec();
            }
        }
        self.position = self.entries.len();
    }

    fn parse_line(line: &str) -> HistoryEntry {
        // Format: "timestamp\tcwd\tcommand" or legacy plain command
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() == 3 {
            if let Ok(ts) = parts[0].parse::<u64>() {
                let cwd = if parts[1].is_empty() { None } else { Some(parts[1].to_string()) };
                return HistoryEntry {
                    command: parts[2].to_string(),
                    timestamp: ts,
                    cwd,
                };
            }
        }
        // Legacy format: plain command
        HistoryEntry {
            command: line.to_string(),
            timestamp: 0,
            cwd: None,
        }
    }

    fn format_entry(entry: &HistoryEntry) -> String {
        let cwd = entry.cwd.as_deref().unwrap_or("");
        format!("{}\t{}\t{}", entry.timestamp, cwd, entry.command)
    }

    pub fn save(&self) {
        if let Ok(mut file) = fs::File::create(&self.file_path) {
            for entry in &self.entries {
                writeln!(file, "{}", Self::format_entry(entry)).ok();
            }
        }
    }

    pub fn add(&mut self, entry: &str) {
        self.add_with_cwd(entry, None);
    }

    pub fn add_with_cwd(&mut self, entry: &str, cwd: Option<&str>) {
        let command = entry.trim().to_string();
        if command.is_empty() { return; }
        if self.entries.last().map(|e| e.command.as_str()) == Some(&command) { return; }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let he = HistoryEntry {
            command: command.clone(),
            timestamp,
            cwd: cwd.map(|s| s.to_string()),
        };

        self.entries.push(he.clone());
        if self.entries.len() > self.max_size {
            self.entries.remove(0);
        }
        self.position = self.entries.len();

        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.file_path) {
            writeln!(file, "{}", Self::format_entry(&he)).ok();
        }
    }

    pub fn last(&self) -> Option<&str> {
        self.entries.last().map(|e| e.command.as_str())
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(|e| e.command.as_str())
    }

    pub fn entries(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.command.as_str()).collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn prev(&mut self) -> Option<&str> {
        if self.position > 0 {
            self.position -= 1;
            Some(&self.entries[self.position].command)
        } else {
            None
        }
    }

    pub fn next(&mut self) -> Option<&str> {
        if !self.entries.is_empty() && self.position + 1 < self.entries.len() {
            self.position += 1;
            Some(&self.entries[self.position].command)
        } else {
            self.position = self.entries.len();
            None
        }
    }

    pub fn reset_position(&mut self) {
        self.position = self.entries.len();
    }

    pub fn search_prefix(&self, prefix: &str) -> Option<&str> {
        if prefix.is_empty() { return None; }
        for entry in self.entries.iter().rev() {
            if entry.command.starts_with(prefix) && entry.command.len() > prefix.len() {
                return Some(&entry.command);
            }
        }
        None
    }

    pub fn search_substring(&self, query: &str) -> Vec<&str> {
        if query.is_empty() { return Vec::new(); }
        self.entries.iter().rev()
            .filter(|e| e.command.contains(query))
            .map(|e| e.command.as_str())
            .collect()
    }

    /// Fuzzy search with metadata: returns (command, matched_indices, timestamp, cwd).
    pub fn search_fuzzy(&self, query: &str) -> Vec<(String, Vec<usize>)> {
        self.search_fuzzy_rich(query).into_iter()
            .map(|(cmd, idx, _, _)| (cmd, idx))
            .collect()
    }

    pub fn search_fuzzy_rich(&self, query: &str) -> Vec<(String, Vec<usize>, u64, Option<String>)> {
        if query.is_empty() { return Vec::new(); }
        let query_lower: Vec<char> = query.to_lowercase().chars().collect();
        let mut results: Vec<(String, Vec<usize>, i32, u64, Option<String>)> = Vec::new();

        for entry in self.entries.iter().rev() {
            let entry_lower: Vec<char> = entry.command.to_lowercase().chars().collect();
            if let Some((indices, score)) = fuzzy_match_score(&query_lower, &entry_lower) {
                if !results.iter().any(|(e, _, _, _, _)| e == &entry.command) {
                    results.push((
                        entry.command.clone(),
                        indices,
                        score,
                        entry.timestamp,
                        entry.cwd.clone(),
                    ));
                }
            }
            if results.len() >= 20 { break; }
        }

        results.sort_by(|a, b| b.2.cmp(&a.2));
        results.into_iter().map(|(cmd, idx, _, ts, cwd)| (cmd, idx, ts, cwd)).collect()
    }

    pub fn format_relative_time(timestamp: u64) -> String {
        if timestamp == 0 { return String::new(); }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let diff = now.saturating_sub(timestamp);
        if diff < 60 { return format!("{}s", diff); }
        if diff < 3600 { return format!("{}m", diff / 60); }
        if diff < 86400 { return format!("{}h", diff / 3600); }
        if diff < 604800 { return format!("{}d", diff / 86400); }
        format!("{}w", diff / 604800)
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
