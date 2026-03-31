/// Auto-suggestion engine: fish-style ghost text from history + z-jump.

use crate::history::History;

/// Given the current buffer, find a suggestion from history or z-jump.
/// Returns the suffix to display as ghost text (the part after the buffer).
pub fn suggest(buffer: &str, history: &History) -> Option<String> {
    if buffer.is_empty() { return None; }

    // 1. Exact prefix match from history (best, current behavior)
    if let Some(entry) = history.search_prefix(buffer) {
        return Some(entry[buffer.len()..].to_string());
    }

    // 2. For "cd " commands, suggest from z-jump database
    if buffer.starts_with("cd ") {
        let query = buffer[3..].trim();
        if !query.is_empty() {
            if let Ok(db) = crate::zjump::get_z_db().lock() {
                if let Some(path) = db.query(&[query]) {
                    // Return the full path as suggestion, replacing the partial arg
                    let current_arg = buffer[3..].to_string();
                    if path.len() > current_arg.len() && path.contains(query) {
                        return Some(path[current_arg.len()..].to_string());
                    }
                    // Or show full path after "cd "
                    let suggestion = format!("{}", &path);
                    if suggestion.starts_with(query) {
                        return Some(suggestion[query.len()..].to_string());
                    }
                    // Fallback: suggest full replacement
                    return Some(format!(" # -> {}", path));
                }
            }
        }
    }

    None
}
