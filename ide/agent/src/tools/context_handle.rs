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
const AGENT_CAPABILITIES: &[&str] = &["rust", "ide", "zed"];

/// Active context thread (id + name), always set together.
pub struct ContextThread {
    pub id: Uuid,
    pub name: String,
}

pub struct ContextHandle {
    pub store: SqliteStore,
    pub workspace_id: Uuid,
    pub context_thread: RwLock<Option<ContextThread>>,
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
        context_thread: RwLock::new(None),
    }))
}

impl ContextHandle {
    /// Set the active context thread (id + name) on this handle atomically.
    pub fn set_context_thread(&self, id: Uuid, name: String) {
        *self.context_thread.write() = Some(ContextThread { id, name });
    }

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

    /// Register a checkin with prism-context (creates/updates the agent session).
    pub async fn checkin(&self) -> anyhow::Result<()> {
        let name = Self::agent_name();
        let thread_id = self.context_thread.read().as_ref().map(|t| t.id);
        let branch = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
                } else {
                    None
                }
            });
        self.store
            .checkin(
                self.workspace_id,
                &name,
                AGENT_CAPABILITIES.iter().map(|s| s.to_string()).collect(),
                thread_id,
                branch,
                None,
            )
            .await?;
        Ok(())
    }

    /// Record a checkout with prism-context (closes the agent session).
    pub async fn checkout(
        &self,
        summary: &str,
        findings: Vec<String>,
        files_touched: Vec<String>,
    ) -> anyhow::Result<()> {
        let name = Self::agent_name();
        self.store
            .checkout(
                self.workspace_id,
                &name,
                summary,
                findings,
                files_touched,
                vec![],
            )
            .await?;
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
        let thread_name = match self.context_thread.read().as_ref().map(|t| t.name.clone()) {
            Some(name) => name,
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

    /// Poll unread messages addressed to this agent. Returns empty vec on error (best-effort).
    pub async fn poll_messages(&self) -> Vec<prism_context::model::Message> {
        let agent_name = Self::agent_name();
        match self
            .store
            .list_messages(self.workspace_id, &agent_name, true)
            .await
        {
            Ok(msgs) => msgs,
            Err(e) => {
                log::debug!("context poll_messages failed: {e}");
                vec![]
            }
        }
    }

    /// Mark all messages to this agent as read.
    pub async fn mark_messages_read(&self) -> anyhow::Result<()> {
        let agent_name = Self::agent_name();
        self.store
            .mark_messages_read(self.workspace_id, &agent_name)
            .await?;
        Ok(())
    }

    /// Create (or debounce-update) a cost spike inbox event.
    pub async fn create_cost_spike_entry(&self, title: &str, near_cap: bool) -> anyhow::Result<()> {
        let thread_id = self.context_thread.read().as_ref().map(|t| t.id);
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
