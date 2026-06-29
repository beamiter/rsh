/// Parser cache: caches AST results for frequently used scripts
/// Useful for interactive shells where users repeat commands
use crate::parser::ast::CompleteCommand;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Cache entry with frequency tracking
#[derive(Clone)]
struct CacheEntry {
    ast: Vec<CompleteCommand>,
    hit_count: u32,
    size: usize, // Size of the input string
}

/// LRU parser cache with frequency tracking
pub struct ParserCache {
    cache: HashMap<String, CacheEntry>,
    max_size: usize,
    max_entries: usize,
}

impl ParserCache {
    pub fn new(max_size: usize, max_entries: usize) -> Self {
        ParserCache {
            cache: HashMap::new(),
            max_size,
            max_entries,
        }
    }

    /// Get from cache
    pub fn get(&mut self, input: &str) -> Option<Vec<CompleteCommand>> {
        if let Some(entry) = self.cache.get_mut(input) {
            entry.hit_count += 1;
            return Some(entry.ast.clone());
        }
        None
    }

    /// Insert into cache
    pub fn insert(&mut self, input: String, ast: Vec<CompleteCommand>) {
        let size = input.len();

        // Don't cache very large inputs
        if size > self.max_size {
            return;
        }

        // Check if we need to evict
        if self.cache.len() >= self.max_entries && !self.cache.contains_key(&input) {
            // Remove least frequently used entry
            if let Some(lfu_key) = self
                .cache
                .iter()
                .min_by_key(|(_, entry)| entry.hit_count)
                .map(|(k, _)| k.clone())
            {
                self.cache.remove(&lfu_key);
            }
        }

        self.cache.insert(
            input,
            CacheEntry {
                ast,
                hit_count: 0,
                size,
            },
        );
    }

    /// Clear the cache
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let entries = self.cache.len();
        let total_size: usize = self.cache.values().map(|e| e.size).sum();
        let total_hits: u32 = self.cache.values().map(|e| e.hit_count).sum();

        CacheStats {
            entries,
            total_size,
            total_hits,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub entries: usize,
    pub total_size: usize,
    pub total_hits: u32,
}

// Thread-local parser cache
thread_local! {
    static PARSER_CACHE: Mutex<ParserCache> =
        Mutex::new(ParserCache::new(4096, 256)); // 256 entries, max 4KB per entry
}

/// Get cached parse result or None
pub fn cache_get(input: &str) -> Option<Vec<CompleteCommand>> {
    PARSER_CACHE.with(|cache| cache.lock().ok().and_then(|mut c| c.get(input)))
}

/// Store parse result in cache
pub fn cache_insert(input: String, ast: Vec<CompleteCommand>) {
    PARSER_CACHE.with(|cache| {
        if let Ok(mut c) = cache.lock() {
            c.insert(input, ast);
        }
    });
}

/// Clear the parser cache
pub fn cache_clear() {
    PARSER_CACHE.with(|cache| {
        if let Ok(mut c) = cache.lock() {
            c.clear();
        }
    });
}

/// Get cache statistics
pub fn cache_stats() -> Option<CacheStats> {
    PARSER_CACHE.with(|cache| cache.lock().ok().map(|c| c.stats()))
}
