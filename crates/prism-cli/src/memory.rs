use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use uglyhat::store::sqlite::SqliteStore;
use uglyhat::store::{MemoryFilters, Store};
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
        let agent_name = std::env::var("UH_AGENT_NAME").unwrap_or_else(|_| "prism".to_string());
        Self {
            store,
            workspace_id,
            pending: Vec::new(),
            agent_name,
        }
    }

    /// Load all memories as a formatted string for system prompt injection.
    pub async fn load(&self) -> String {
        let Some(store) = &self.store else {
            return String::new();
        };
        let Some(ws_id) = self.workspace_id else {
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

/// Walk up from cwd looking for `.uglyhat.db` and `.uglyhat.json` in one pass.
fn discover_uglyhat() -> (Option<std::path::PathBuf>, Option<Uuid>) {
    let mut dir = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return (None, None),
    };
    let mut db_path = None;
    let mut ws_id = None;
    loop {
        if db_path.is_none() {
            let db = dir.join(".uglyhat.db");
            if db.exists() {
                db_path = Some(db);
            }
        }
        if ws_id.is_none() {
            let config_path = dir.join(".uglyhat.json");
            if config_path.exists()
                && let Ok(data) = std::fs::read_to_string(&config_path)
                && let Ok(config) = serde_json::from_str::<serde_json::Value>(&data)
            {
                ws_id = config["workspace_id"].as_str().and_then(|s| s.parse().ok());
            }
        }
        if db_path.is_some() && ws_id.is_some() {
            break;
        }
        if !dir.pop() {
            break;
        }
    }
    (db_path, ws_id)
}
