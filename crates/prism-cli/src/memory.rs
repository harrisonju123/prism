use anyhow::Result;
use prism_context::store::sqlite::SqliteStore;
use prism_context::store::{MemoryFilters, Store};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

/// Context-store backed memory manager using uglyhat's SqliteStore.
/// Falls back to in-memory buffering if the store can't be opened.
pub struct MemoryManager {
    store: Option<Arc<SqliteStore>>,
    workspace_id: Option<Uuid>,
    pending: Vec<(String, String)>,
    agent_name: String,
}

impl MemoryManager {
    /// Create a new MemoryManager backed by an uglyhat store.
    /// If `store` is None, memories are buffered in-memory and lost on exit.
    pub fn new(store: Option<Arc<SqliteStore>>, workspace_id: Option<Uuid>) -> Self {
        let agent_name = crate::config::agent_name_from_env();
        Self {
            store,
            workspace_id,
            pending: Vec::new(),
            agent_name,
        }
    }

    /// Load all memories as a formatted string for system prompt injection.
    pub async fn load(&self) -> String {
        Self::load_from(self.store.as_deref(), self.workspace_id).await
    }

    /// Load memories without borrowing self — allows callers to release the
    /// RefCell borrow before awaiting (avoids borrow-across-await).
    pub async fn load_from(store: Option<&SqliteStore>, workspace_id: Option<Uuid>) -> String {
        let Some(store) = store else {
            return String::new();
        };
        let Some(ws_id) = workspace_id else {
            return String::new();
        };

        let memories = store
            .load_memories(ws_id, MemoryFilters::default())
            .await
            .unwrap_or_default();

        if memories.is_empty() {
            return String::new();
        }

        let mut buf = String::new();
        for m in &memories {
            buf.push_str(&format!("## {}\n{}\n\n", m.key, m.value));
        }
        buf
    }

    /// Queue a memory for saving. Flushes immediately if store is available.
    pub async fn save(&self, key: &str, value: &str) -> Result<()> {
        if let (Some(store), Some(ws_id)) = (&self.store, self.workspace_id) {
            store
                .save_memory(ws_id, key, value, None, &self.agent_name, vec![])
                .await
                .map_err(|e| anyhow::anyhow!("save memory: {e}"))?;
        }
        Ok(())
    }

    /// Buffer a memory for later flush (sync path for tool calls).
    pub fn append(&mut self, key: String, value: String) {
        self.pending.push((key, value));
    }

    /// Flush any pending memories to the store, clearing the buffer.
    pub async fn flush(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let Some(store) = &self.store else {
            self.pending.clear();
            return Ok(());
        };
        let Some(ws_id) = self.workspace_id else {
            self.pending.clear();
            return Ok(());
        };

        for (key, value) in self.pending.drain(..) {
            let _ = store
                .save_memory(ws_id, &key, &value, None, &self.agent_name, vec![])
                .await;
        }
        Ok(())
    }

    pub fn store(&self) -> Option<&Arc<SqliteStore>> {
        self.store.as_ref()
    }

    pub fn workspace_id(&self) -> Option<Uuid> {
        self.workspace_id
    }
}

/// Try to open an uglyhat store and discover the workspace.
/// Returns (store, workspace_id) if successful, or (None, None) if no uglyhat DB found.
pub async fn open_context_store(
    db_path: Option<&Path>,
) -> (Option<Arc<SqliteStore>>, Option<Uuid>) {
    let (discovered_db, discovered_ws) = discover_uglyhat();

    let path = match db_path {
        Some(p) => Some(p.to_path_buf()),
        None => discovered_db,
    };

    let Some(path) = path else {
        return (None, None);
    };

    match SqliteStore::open(&path.to_string_lossy()).await {
        Ok(store) => (Some(Arc::new(store)), discovered_ws),
        Err(e) => {
            tracing::warn!("failed to open uglyhat store at {}: {e}", path.display());
            (None, None)
        }
    }
}

/// Discover uglyhat config and database path from the current directory.
fn discover_uglyhat() -> (Option<std::path::PathBuf>, Option<Uuid>) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return (None, None),
    };
    let Some(config_path) = prism_context::config::find_config(&cwd) else {
        return (None, None);
    };
    let config = match prism_context::config::load_config(&config_path) {
        Ok(c) => c,
        Err(_) => return (None, None),
    };
    let db_path = prism_context::config::resolve_db_path(&config_path, &config);
    let ws_id = config.workspace_id.parse().ok();
    (Some(db_path), ws_id)
}

/// Keywords indicating error resolution patterns in session content.
pub const RESOLUTION_KEYWORDS: &[&str] =
    &["fixed", "resolved", "workaround", "root cause", "solution"];

/// Auto-extract memories from session data on checkout.
/// Pure heuristics — no LLM involved.
pub async fn auto_extract_memories(
    store: &dyn Store,
    workspace_id: Uuid,
    agent_name: &str,
    next_steps: &[String],
    files_touched: &[String],
    findings: &[String],
    thread_name: Option<&str>,
) -> Vec<String> {
    let mut saved = Vec::new();
    let thread_tag = thread_name.unwrap_or("global");

    // 1. Next-steps → memories
    for step in next_steps {
        let hash = &format!("{:x}", dedup_hash(step))[..8];
        let key = format!("next_step:{thread_tag}:{hash}");
        if store
            .save_memory(
                workspace_id,
                &key,
                step,
                None,
                agent_name,
                vec!["next_step".to_string(), thread_tag.to_string()],
            )
            .await
            .is_ok()
        {
            saved.push(key);
        }
    }

    // 2. File co-modification patterns
    if files_touched.len() >= 3 {
        let dirs: HashSet<String> = files_touched
            .iter()
            .filter_map(|f| {
                let parts: Vec<&str> = f.rsplitn(2, '/').collect();
                if parts.len() == 2 {
                    Some(parts[1].to_string())
                } else {
                    None
                }
            })
            .collect();
        if dirs.len() >= 3 {
            let mut sorted_dirs: Vec<&str> = dirs.iter().map(|s| s.as_str()).collect();
            sorted_dirs.sort();
            let hash = &format!("{:x}", dedup_hash(&sorted_dirs.join(",")))[..8];
            let key = format!("file_pattern:{hash}");
            let value = format!(
                "Files spanning {} directories modified together: {}",
                dirs.len(),
                files_touched.join(", ")
            );
            if store
                .save_memory(
                    workspace_id,
                    &key,
                    &value,
                    None,
                    agent_name,
                    vec!["file_pattern".to_string()],
                )
                .await
                .is_ok()
            {
                saved.push(key);
            }
        }
    }

    // 3. Error resolution patterns
    let resolution_keywords = RESOLUTION_KEYWORDS;
    for finding in findings {
        let lower = finding.to_lowercase();
        if resolution_keywords.iter().any(|kw| lower.contains(kw)) {
            let hash = &format!("{:x}", dedup_hash(finding))[..8];
            let key = format!("resolution:{hash}");
            if store
                .save_memory(
                    workspace_id,
                    &key,
                    finding,
                    None,
                    agent_name,
                    vec!["resolution".to_string(), thread_tag.to_string()],
                )
                .await
                .is_ok()
            {
                saved.push(key);
            }
        }
    }

    saved
}

/// FNV-1a hash for stable dedup keys across Rust versions.
fn dedup_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for byte in s.as_bytes() {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
