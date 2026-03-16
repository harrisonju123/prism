use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;

const MAX_RESULT_BYTES: usize = 512 * 1024;
const CACHE_PREFIX: &str = "[cached — unchanged since last read]\n";

struct CacheEntry {
    result: String,
}

pub struct ToolResultCache {
    entries: DashMap<String, CacheEntry>,
    /// path → list of cache keys that reference this path (for invalidation)
    path_index: DashMap<String, Vec<String>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl ToolResultCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: DashMap::new(),
            path_index: DashMap::new(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        })
    }

    /// Look up a cached result. Returns the result with a `[cached]` prefix so the
    /// LLM knows it is seeing a previously-fetched value.
    pub fn get(&self, key: &str) -> Option<String> {
        if let Some(entry) = self.entries.get(key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            Some(entry.result.clone())
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Store a result. `path` is the canonical path associated with the entry (for
    /// invalidation). Results larger than 512 KB are not cached. The cache prefix is
    /// prepended once here so `get` pays only the cost of a clone, not a format.
    pub fn insert(&self, key: String, result: String, path: String) {
        if result.len() > MAX_RESULT_BYTES {
            return;
        }
        let prefixed = format!("{CACHE_PREFIX}{result}");
        self.entries.insert(key.clone(), CacheEntry { result: prefixed });
        self.path_index.entry(path).or_default().push(key);
    }

    /// Invalidate all entries that reference `path` exactly (e.g. after write_file/edit_file).
    pub fn invalidate_path(&self, path: &str) {
        if let Some((_, keys)) = self.path_index.remove(path) {
            for key in keys {
                self.entries.remove(&key);
            }
        }
    }

    /// Invalidate glob/grep/list_dir entries whose search root is a parent directory of
    /// the written file, since the directory listing may have changed.
    pub fn invalidate_dir_containing(&self, file_path: &str) {
        let file = std::path::Path::new(file_path);

        let affected_paths: Vec<String> = self
            .path_index
            .iter()
            .filter_map(|entry| {
                let dir = std::path::Path::new(entry.key());
                if dir != file && file.starts_with(dir) {
                    Some(entry.key().clone())
                } else {
                    None
                }
            })
            .collect();

        for path in &affected_paths {
            if let Some((_, keys)) = self.path_index.remove(path) {
                for key in keys {
                    self.entries.remove(&key);
                }
            }
        }
    }

    /// Returns (hits, misses) for the session — used for tracing at session end.
    pub fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }
}
