use std::sync::Arc;

use gpui::App;
use gpui_tokio::Tokio;
use parking_lot::RwLock;
use project::Project;
use gpui::Entity;
use prism_context::config;
use prism_context::store::sqlite::SqliteStore;
use uuid::Uuid;

pub const AGENT_SOURCE: &str = "zed-agent";

pub struct ContextHandle {
    pub store: SqliteStore,
    pub workspace_id: Uuid,
    pub context_thread_id: RwLock<Option<Uuid>>,
}

pub fn try_init_context_handle(project: &Entity<Project>, cx: &App) -> Option<Arc<ContextHandle>> {
    let worktree = project.read(cx).worktrees(cx).next()?;
    let root = worktree.read(cx).abs_path().to_path_buf();

    let config_path = config::find_config(&root)?;
    let cfg = config::load_config(&config_path)
        .map_err(|e| log::warn!("prism-context: failed to load config: {e}"))
        .ok()?;

    let workspace_id: Uuid = cfg
        .workspace_id
        .parse()
        .map_err(|e| log::warn!("prism-context: invalid workspace_id: {e}"))
        .ok()?;

    let db_path = config::resolve_db_path(&config_path, &cfg);
    let db_path_str = db_path.to_string_lossy().to_string();

    let store = Tokio::handle(cx)
        .block_on(SqliteStore::open(&db_path_str))
        .map_err(|e| log::warn!("prism-context: failed to open store: {e}"))
        .ok()?;

    Some(Arc::new(ContextHandle {
        store,
        workspace_id,
        context_thread_id: RwLock::new(None),
    }))
}
