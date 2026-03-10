pub mod sqlite;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::Result;
use crate::model::*;

#[derive(Debug, Default)]
pub struct MemoryFilters {
    pub thread_id: Option<Uuid>,
    pub thread_name: Option<String>,
    pub tags: Option<Vec<String>>,
    pub global_only: bool,
}

#[derive(Debug, Default)]
pub struct ActivityFilters {
    pub since: Option<DateTime<Utc>>,
    pub actor: Option<String>,
    pub limit: i64,
}

#[async_trait]
pub trait Store: Send + Sync {
    // --- Workspace (2) ---
    async fn init_workspace(&self, name: &str, desc: &str) -> Result<Workspace>;
    async fn get_workspace(&self, id: Uuid) -> Result<Workspace>;

    // --- Thread (4) ---
    async fn create_thread(
        &self,
        workspace_id: Uuid,
        name: &str,
        desc: &str,
        tags: Vec<String>,
    ) -> Result<Thread>;
    async fn get_thread(&self, workspace_id: Uuid, name: &str) -> Result<Thread>;
    async fn list_threads(
        &self,
        workspace_id: Uuid,
        status: Option<ThreadStatus>,
    ) -> Result<Vec<Thread>>;
    async fn archive_thread(&self, workspace_id: Uuid, name: &str) -> Result<Thread>;

    // --- Memory (3) ---
    async fn save_memory(
        &self,
        workspace_id: Uuid,
        key: &str,
        value: &str,
        thread_id: Option<Uuid>,
        source: &str,
        tags: Vec<String>,
    ) -> Result<Memory>;
    async fn load_memories(
        &self,
        workspace_id: Uuid,
        filters: MemoryFilters,
    ) -> Result<Vec<Memory>>;
    async fn delete_memory(&self, workspace_id: Uuid, key: &str) -> Result<()>;

    // --- Decision (2) ---
    async fn save_decision(
        &self,
        workspace_id: Uuid,
        title: &str,
        content: &str,
        thread_id: Option<Uuid>,
        tags: Vec<String>,
    ) -> Result<Decision>;
    async fn list_decisions(
        &self,
        workspace_id: Uuid,
        thread_id: Option<Uuid>,
        tags: Option<Vec<String>>,
    ) -> Result<Vec<Decision>>;

    // --- Agent (3) ---
    async fn checkin(
        &self,
        workspace_id: Uuid,
        name: &str,
        capabilities: Vec<String>,
        thread_id: Option<Uuid>,
    ) -> Result<CheckinContext>;
    async fn checkout(
        &self,
        workspace_id: Uuid,
        name: &str,
        summary: &str,
        findings: Vec<String>,
        files_touched: Vec<String>,
        next_steps: Vec<String>,
    ) -> Result<AgentSession>;
    async fn list_agents(&self, workspace_id: Uuid) -> Result<Vec<AgentStatus>>;

    // --- Context (2) ---
    async fn recall_thread(&self, workspace_id: Uuid, thread_name: &str) -> Result<ThreadContext>;
    async fn recall_by_tags(
        &self,
        workspace_id: Uuid,
        tags: Vec<String>,
        since: Option<DateTime<Utc>>,
    ) -> Result<RecallResult>;

    // --- Activity (1) ---
    async fn list_activity(
        &self,
        workspace_id: Uuid,
        filters: ActivityFilters,
    ) -> Result<Vec<ActivityEntry>>;

    // --- Snapshot (1) ---
    async fn create_snapshot(&self, workspace_id: Uuid, label: &str) -> Result<Snapshot>;

    // --- Overview (1) ---
    async fn get_workspace_overview(&self, workspace_id: Uuid) -> Result<WorkspaceOverview>;
}
