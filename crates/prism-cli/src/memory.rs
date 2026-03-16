use anyhow::Result;
use prism_context::store::sqlite::SqliteStore;
use prism_context::store::{MemoryFilters, Store};
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

/// Context-store backed memory manager using PrisM's SqliteStore.
/// Falls back to in-memory buffering if the store can't be opened.
pub struct MemoryManager {
    store: Option<Arc<SqliteStore>>,
    workspace_id: Option<Uuid>,
    pending: Vec<(String, String)>,
    agent_name: String,
}

impl MemoryManager {
    /// Create a new MemoryManager backed by a context store.
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

/// Try to open a context store and discover the workspace.
/// Returns (store, workspace_id) if successful, or (None, None) if no context DB found.
pub async fn open_context_store(
    db_path: Option<&Path>,
) -> (Option<Arc<SqliteStore>>, Option<Uuid>) {
    let (discovered_db, discovered_ws) = discover_context_store();

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
            tracing::warn!("failed to open context store at {}: {e}", path.display());
            (None, None)
        }
    }
}

/// Discover context config and database path from the current directory.
fn discover_context_store() -> (Option<std::path::PathBuf>, Option<Uuid>) {
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

// Re-exported from prism-context so IDE and CLI share the same implementation.
pub use prism_context::memory_extract::{RESOLUTION_KEYWORDS, auto_extract_memories};
