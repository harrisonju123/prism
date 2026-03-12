pub mod chunking;
pub mod db;
pub mod embedding;
pub mod indexer;
pub mod search;

use anyhow::Result;
use db::SemanticIndexDb;
use embedding::EmbeddingProvider;
use gpui::Global;
use indexer::{IndexSettings, IndexStats};
use paths::embeddings_dir;
use search::SearchResult;
use sqlez::thread_safe_connection::ThreadSafeConnection;
use sqlez_macros::sql;
use std::{path::Path, sync::Arc};

const DB_INITIALIZE_QUERY: &str = sql!(
    PRAGMA journal_mode=WAL;
    PRAGMA busy_timeout=500;
    PRAGMA case_sensitive_like=TRUE;
    PRAGMA synchronous=NORMAL;
);

const CONNECTION_INITIALIZE_QUERY: &str = sql!(
    PRAGMA foreign_keys=TRUE;
);

/// Central semantic index for a single project root / worktree.
///
/// Stored as a gpui global so agent tools can access it from the app context without
/// threading an explicit reference through every call site.
pub struct SemanticIndex {
    db: ThreadSafeConnection,
    provider: Arc<dyn EmbeddingProvider>,
    pub worktree_id: i64,
}

impl Clone for SemanticIndex {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            provider: self.provider.clone(),
            worktree_id: self.worktree_id,
        }
    }
}

impl Global for SemanticIndex {}

impl SemanticIndex {
    /// Open (or create) the semantic index database for the given `worktree_id`.
    ///
    /// The DB lives at `embeddings_dir()/<worktree_id>.db` and is initialised with
    /// the `SemanticIndexDb` migrations on first open.
    pub async fn open(worktree_id: i64, provider: Arc<dyn EmbeddingProvider>) -> Result<Self> {
        let db_dir = embeddings_dir();
        std::fs::create_dir_all(db_dir)?;
        let db_path = db_dir.join(format!("{worktree_id}.db"));
        let db_path_str = db_path.to_string_lossy().into_owned();

        let db = ThreadSafeConnection::builder::<SemanticIndexDb>(&db_path_str, true)
            .with_db_initialization_query(DB_INITIALIZE_QUERY)
            .with_connection_initialize_query(CONNECTION_INITIALIZE_QUERY)
            .build()
            .await?;

        Ok(Self { db, provider, worktree_id })
    }

    /// Index all files under `root_path`. Unchanged files are skipped.
    pub async fn index_directory(
        &self,
        root_path: &Path,
        settings: Option<IndexSettings>,
    ) -> Result<IndexStats> {
        indexer::index_directory(
            &self.db,
            self.provider.as_ref(),
            self.worktree_id,
            root_path,
            &settings.unwrap_or_default(),
        )
        .await
    }

    /// Semantic search: embed `query` and return the top `limit` matching chunks.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        search::search(&self.db, self.provider.as_ref(), query, limit).await
    }
}
