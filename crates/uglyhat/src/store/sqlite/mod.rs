#[macro_use]
pub mod types;
mod activity;
mod agent;
mod context;
mod decision;
mod memory;
mod migrate;
mod snapshot;
#[cfg(test)]
mod tests;
mod thread;
mod workspace;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

use crate::error::Result;
use crate::model::*;
use crate::store::{ActivityFilters, MemoryFilters, Store};

pub struct SqliteStore {
    pub(crate) pool: SqlitePool,
}

#[cfg(test)]
impl SqliteStore {
    pub(crate) async fn open_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .map_err(|e| crate::error::Error::Internal(format!("open memory sqlite: {e}")))?;

        migrate::run_migrations(&pool).await?;

        Ok(Self { pool })
    }
}

impl SqliteStore {
    pub async fn open(path: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .map_err(|e| crate::error::Error::Internal(format!("open sqlite: {e}")))?;

        migrate::run_migrations(&pool).await?;

        Ok(Self { pool })
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn init_workspace(&self, name: &str, desc: &str) -> Result<Workspace> {
        self.init_workspace_impl(name, desc).await
    }

    async fn get_workspace(&self, id: Uuid) -> Result<Workspace> {
        self.get_workspace_impl(id).await
    }

    async fn create_thread(
        &self,
        workspace_id: Uuid,
        name: &str,
        desc: &str,
        tags: Vec<String>,
    ) -> Result<Thread> {
        self.create_thread_impl(workspace_id, name, desc, tags)
            .await
    }

    async fn get_thread(&self, workspace_id: Uuid, name: &str) -> Result<Thread> {
        self.get_thread_impl(workspace_id, name).await
    }

    async fn list_threads(
        &self,
        workspace_id: Uuid,
        status: Option<ThreadStatus>,
    ) -> Result<Vec<Thread>> {
        self.list_threads_impl(workspace_id, status).await
    }

    async fn archive_thread(&self, workspace_id: Uuid, name: &str) -> Result<Thread> {
        self.archive_thread_impl(workspace_id, name).await
    }

    async fn save_memory(
        &self,
        workspace_id: Uuid,
        key: &str,
        value: &str,
        thread_id: Option<Uuid>,
        source: &str,
        tags: Vec<String>,
    ) -> Result<Memory> {
        self.save_memory_impl(workspace_id, key, value, thread_id, source, tags)
            .await
    }

    async fn load_memories(
        &self,
        workspace_id: Uuid,
        filters: MemoryFilters,
    ) -> Result<Vec<Memory>> {
        self.load_memories_impl(workspace_id, filters).await
    }

    async fn delete_memory(&self, workspace_id: Uuid, key: &str) -> Result<()> {
        self.delete_memory_impl(workspace_id, key).await
    }

    async fn save_decision(
        &self,
        workspace_id: Uuid,
        title: &str,
        content: &str,
        thread_id: Option<Uuid>,
        tags: Vec<String>,
    ) -> Result<Decision> {
        self.save_decision_impl(workspace_id, title, content, thread_id, tags)
            .await
    }

    async fn list_decisions(
        &self,
        workspace_id: Uuid,
        thread_id: Option<Uuid>,
        tags: Option<Vec<String>>,
    ) -> Result<Vec<Decision>> {
        self.list_decisions_impl(workspace_id, thread_id, tags)
            .await
    }

    async fn checkin(
        &self,
        workspace_id: Uuid,
        name: &str,
        capabilities: Vec<String>,
        thread_id: Option<Uuid>,
    ) -> Result<CheckinContext> {
        self.checkin_impl(workspace_id, name, capabilities, thread_id)
            .await
    }

    async fn checkout(
        &self,
        workspace_id: Uuid,
        name: &str,
        summary: &str,
        findings: Vec<String>,
        files_touched: Vec<String>,
        next_steps: Vec<String>,
    ) -> Result<AgentSession> {
        self.checkout_impl(
            workspace_id,
            name,
            summary,
            findings,
            files_touched,
            next_steps,
        )
        .await
    }

    async fn list_agents(&self, workspace_id: Uuid) -> Result<Vec<AgentStatus>> {
        self.list_agents_impl(workspace_id).await
    }

    async fn recall_thread(&self, workspace_id: Uuid, thread_name: &str) -> Result<ThreadContext> {
        self.recall_thread_impl(workspace_id, thread_name).await
    }

    async fn recall_by_tags(
        &self,
        workspace_id: Uuid,
        tags: Vec<String>,
        since: Option<DateTime<Utc>>,
    ) -> Result<RecallResult> {
        self.recall_by_tags_impl(workspace_id, tags, since).await
    }

    async fn list_activity(
        &self,
        workspace_id: Uuid,
        filters: ActivityFilters,
    ) -> Result<Vec<ActivityEntry>> {
        self.list_activity_impl(workspace_id, filters).await
    }

    async fn create_snapshot(&self, workspace_id: Uuid, label: &str) -> Result<Snapshot> {
        self.create_snapshot_impl(workspace_id, label).await
    }

    async fn get_workspace_overview(&self, workspace_id: Uuid) -> Result<WorkspaceOverview> {
        self.get_workspace_overview_impl(workspace_id).await
    }
}
