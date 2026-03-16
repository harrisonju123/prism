use std::sync::Arc;

use gpui::App;
use gpui_tokio::Tokio;
use parking_lot::RwLock;
use project::Project;
use gpui::Entity;
use prism_context::config;
use prism_context::model::{InboxEntryType, InboxSeverity};
use prism_context::store::sqlite::SqliteStore;
use prism_context::store::Store as _;
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

impl ContextHandle {
    /// Returns the agent name from env (PRISM_AGENT_NAME or UH_AGENT_NAME fallback).
    pub fn agent_name() -> String {
        std::env::var("PRISM_AGENT_NAME")
            .or_else(|_| std::env::var("UH_AGENT_NAME"))
            .unwrap_or_else(|_| "zed-agent".to_string())
    }

    /// Send a heartbeat to mark this agent as alive.
    pub async fn heartbeat(&self) -> anyhow::Result<()> {
        let name = Self::agent_name();
        self.store.heartbeat(self.workspace_id, &name).await?;
        Ok(())
    }

    /// Check if a file is claimed by another agent.
    /// Returns `Some(owner_name)` if blocked, `None` if the path is free or owned by us.
    pub async fn check_file_claim(&self, path: &str) -> anyhow::Result<Option<String>> {
        let agent_name = Self::agent_name();
        match self.store.check_file_claim(self.workspace_id, path).await? {
            Some(claim) if claim.agent_name != agent_name => Ok(Some(claim.agent_name)),
            _ => Ok(None),
        }
    }

    /// Claim a file for this agent (TTL = 1 hour).
    pub async fn claim_file(&self, path: &str) -> anyhow::Result<()> {
        let agent_name = Self::agent_name();
        self.store
            .claim_file(self.workspace_id, &agent_name, path, Some(3600))
            .await?;
        Ok(())
    }

    /// Check thread-scoped guardrails. Returns `Some(denial_reason)` if denied.
    pub async fn check_guardrail(
        &self,
        tool_name: &str,
        file_path: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        let thread_id = self.context_thread_id.read().clone();
        let thread_name = match thread_id {
            Some(id) => id.to_string(),
            None => return Ok(None),
        };
        let agent_name = Self::agent_name();
        let check = self
            .store
            .check_guardrail(
                self.workspace_id,
                &thread_name,
                &agent_name,
                tool_name,
                file_path,
            )
            .await?;
        if !check.allowed {
            Ok(Some(
                check
                    .reason
                    .unwrap_or_else(|| "access denied by guardrail".to_string()),
            ))
        } else {
            Ok(None)
        }
    }

    /// Create (or debounce-update) a cost spike inbox event.
    pub async fn create_cost_spike_entry(&self, title: &str, near_cap: bool) -> anyhow::Result<()> {
        let thread_id = self.context_thread_id.read().clone();
        let severity = if near_cap {
            InboxSeverity::Critical
        } else {
            InboxSeverity::Warning
        };
        let agent_name = Self::agent_name();
        self.store
            .create_or_update_inbox_entry(
                self.workspace_id,
                InboxEntryType::CostSpike,
                title,
                "",
                severity,
                Some(&agent_name),
                thread_id.as_ref().map(|_| "thread"),
                thread_id,
                Some(300),
            )
            .await?;
        Ok(())
    }
}
