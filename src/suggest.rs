/// Auto-suggestion engine: fish-style ghost text from history.

use crate::history::History;

/// Given the current buffer, find a suggestion from history.
/// Returns the suffix to display as ghost text (the part after the buffer).
pub fn suggest(buffer: &str, history: &History) -> Option<String> {
    if buffer.is_empty() { return None; }
    if let Some(entry) = history.search_prefix(buffer) {
        Some(entry[buffer.len()..].to_string())
    } else {
        None
    }
}
