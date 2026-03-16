use std::sync::Arc;

use chrono::{DateTime, Utc};
use gpui::App;
use gpui::Entity;
use gpui_tokio::Tokio;
use parking_lot::RwLock;
use project::Project;
use prism_context::config;
use prism_context::model::{
    Decision, DecisionScope, InboxEntry, InboxEntryType, InboxSeverity, Memory, Message,
    RecallResult, Snapshot, Thread, ThreadContext, ThreadStatus, WorkspaceOverview,
};
use prism_context::store::sqlite::SqliteStore;
use prism_context::store::{MemoryFilters, Store as _};
use uuid::Uuid;

pub const AGENT_SOURCE: &str = "zed-agent";
const AGENT_CAPABILITIES: &[&str] = &["rust", "ide", "zed"];

/// Active context thread (id + name), always set together.
pub struct ContextThread {
    pub id: Uuid,
    pub name: String,
}

pub struct ContextHandle {
    store: SqliteStore,
    pub workspace_id: Uuid,
    pub context_thread: RwLock<Option<ContextThread>>,
}

pub fn try_init_context_handle(project: &Entity<Project>, cx: &App) -> Option<Arc<ContextHandle>> {
    let worktree = project.read(cx).worktrees(cx).next()?;
    let root = worktree.read(cx).abs_path().to_path_buf();

    // Resolve config path — auto-init if none found but we're in a git repo.
    let config_path = match config::find_config(&root) {
        Some(p) => p,
        None => {
            if !root.join(".git").exists() {
                return None;
            }
            let name = root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("workspace")
                .to_string();
            match Tokio::handle(cx).block_on(config::auto_init(&root, &name)) {
                Ok((path, _)) => {
                    log::info!("prism-context: auto-initialized workspace at {}", path.display());
                    path
                }
                Err(e) => {
                    log::warn!("prism-context: auto-init failed: {e}");
                    return None;
                }
            }
        }
    };

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
    pub async fn poll_messages(&self) -> Vec<Message> {
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

    /// Reap agents whose heartbeat is older than `timeout_secs`.
    pub async fn reap_dead_agents(&self, timeout_secs: i64) -> anyhow::Result<Vec<String>> {
        self.store
            .reap_dead_agents(self.workspace_id, timeout_secs)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
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

    // --- New wrapper methods (Phase 1) ---

    /// Save (upsert) a memory. Auto-injects workspace_id, thread_id, and source.
    pub async fn save_memory(
        &self,
        key: &str,
        value: &str,
        tags: Vec<String>,
    ) -> anyhow::Result<Memory> {
        let thread_id = self.context_thread.read().as_ref().map(|t| t.id);
        Ok(self.store
            .save_memory(self.workspace_id, key, value, thread_id, AGENT_SOURCE, tags)
            .await?)
    }

    /// Delete a memory by key.
    pub async fn delete_memory(&self, key: &str) -> anyhow::Result<()> {
        Ok(self.store.delete_memory(self.workspace_id, key).await?)
    }

    /// Load memories with optional filters.
    pub async fn load_memories(&self, filters: MemoryFilters) -> anyhow::Result<Vec<Memory>> {
        Ok(self.store.load_memories(self.workspace_id, filters).await?)
    }

    /// Save a decision. Caller provides thread_id (resolved from context_thread if needed).
    pub async fn save_decision(
        &self,
        title: &str,
        content: &str,
        thread_id: Option<Uuid>,
        tags: Vec<String>,
        scope: DecisionScope,
    ) -> anyhow::Result<Decision> {
        Ok(self.store
            .save_decision(self.workspace_id, title, content, thread_id, tags, scope)
            .await?)
    }

    /// Create a new context thread.
    pub async fn create_thread(
        &self,
        name: &str,
        desc: &str,
        tags: Vec<String>,
    ) -> anyhow::Result<Thread> {
        Ok(self.store.create_thread(self.workspace_id, name, desc, tags).await?)
    }

    /// List threads, optionally filtered by status.
    pub async fn list_threads(&self, status: Option<ThreadStatus>) -> anyhow::Result<Vec<Thread>> {
        Ok(self.store.list_threads(self.workspace_id, status).await?)
    }

    /// Archive a thread by name.
    pub async fn archive_thread(&self, name: &str) -> anyhow::Result<Thread> {
        Ok(self.store.archive_thread(self.workspace_id, name).await?)
    }

    /// Recall full context for a named thread.
    pub async fn recall_thread(&self, thread_name: &str) -> anyhow::Result<ThreadContext> {
        Ok(self.store.recall_thread(self.workspace_id, thread_name).await?)
    }

    /// Recall memories and decisions by tags, with optional recency filter.
    pub async fn recall_by_tags(
        &self,
        tags: Vec<String>,
        since: Option<DateTime<Utc>>,
    ) -> anyhow::Result<RecallResult> {
        Ok(self.store.recall_by_tags(self.workspace_id, tags, since).await?)
    }

    /// Get workspace overview (threads, memories, agents, sessions).
    pub async fn get_workspace_overview(&self) -> anyhow::Result<WorkspaceOverview> {
        Ok(self.store.get_workspace_overview(self.workspace_id).await?)
    }

    /// Capture a point-in-time snapshot of workspace state.
    pub async fn create_snapshot(&self, label: &str) -> anyhow::Result<Snapshot> {
        Ok(self.store.create_snapshot(self.workspace_id, label).await?)
    }

    /// List workspace snapshots in reverse chronological order.
    pub async fn list_snapshots(&self, limit: Option<i64>) -> anyhow::Result<Vec<Snapshot>> {
        Ok(self.store.list_snapshots(self.workspace_id, limit).await?)
    }

    /// Send a message to another agent. Auto-injects workspace_id and from_agent.
    pub async fn send_message(
        &self,
        to_agent: &str,
        content: &str,
        conversation_id: Option<Uuid>,
    ) -> anyhow::Result<Message> {
        let from_agent = Self::agent_name();
        Ok(self.store
            .send_message(self.workspace_id, &from_agent, to_agent, content, conversation_id)
            .await?)
    }

    /// Create an inbox entry. Auto-injects workspace_id and AGENT_SOURCE.
    pub async fn create_inbox_entry(
        &self,
        entry_type: InboxEntryType,
        title: &str,
        body: &str,
        severity: InboxSeverity,
        ref_type: Option<&str>,
        ref_id: Option<Uuid>,
    ) -> anyhow::Result<InboxEntry> {
        Ok(self.store
            .create_inbox_entry(
                self.workspace_id,
                entry_type,
                title,
                body,
                severity,
                Some(AGENT_SOURCE),
                ref_type,
                ref_id,
            )
            .await?)
    }

    /// Auto-extract memories from session output. Auto-injects workspace_id and agent_name.
    pub async fn auto_extract_memories(
        &self,
        next_steps: &[String],
        files_touched: &[String],
        findings: &[String],
        thread_name: Option<&str>,
    ) {
        let agent_name = Self::agent_name();
        prism_context::memory_extract::auto_extract_memories(
            &self.store,
            self.workspace_id,
            &agent_name,
            next_steps,
            files_touched,
            findings,
            thread_name,
        )
        .await;
    }
}
