use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::SqliteStore;
use crate::error::Result;
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn recall_thread_impl(
        &self,
        workspace_id: Uuid,
        thread_name: &str,
    ) -> Result<ThreadContext> {
        let thread = self.get_thread_impl(workspace_id, thread_name).await?;
        let thread_id = thread.id;

        let memories = self
            .load_memories_impl(
                workspace_id,
                crate::store::MemoryFilters {
                    thread_id: Some(thread_id),
                    ..Default::default()
                },
            )
            .await?;

        let decisions = self
            .list_decisions_impl(workspace_id, Some(thread_id), None)
            .await?;

        let recent_sessions = self.fetch_thread_sessions(workspace_id, thread_id).await?;

        let recent_activity = self.fetch_thread_activity(workspace_id, thread_id).await?;

        Ok(ThreadContext {
            thread,
            memories,
            decisions,
            recent_sessions,
            recent_activity,
        })
    }

    pub(crate) async fn recall_by_tags_impl(
        &self,
        workspace_id: Uuid,
        tags: Vec<String>,
        since: Option<DateTime<Utc>>,
    ) -> Result<RecallResult> {
        let memories = self
            .load_memories_impl(
                workspace_id,
                crate::store::MemoryFilters {
                    tags: Some(tags.clone()),
                    ..Default::default()
                },
            )
            .await?;

        let decisions = self
            .list_decisions_impl(workspace_id, None, Some(tags))
            .await?;

        let activity = self
            .list_activity_impl(
                workspace_id,
                crate::store::ActivityFilters {
                    since,
                    limit: 50,
                    ..Default::default()
                },
            )
            .await?;

        Ok(RecallResult {
            memories,
            decisions,
            activity,
        })
    }

    pub(crate) async fn get_workspace_overview_impl(
        &self,
        workspace_id: Uuid,
    ) -> Result<WorkspaceOverview> {
        let workspace = self.get_workspace_impl(workspace_id).await?;

        let active_threads = self
            .list_threads_impl(workspace_id, Some(ThreadStatus::Active))
            .await?;

        let recent_memories = self
            .load_memories_impl(workspace_id, crate::store::MemoryFilters::default())
            .await?;

        let recent_decisions = self.list_decisions_impl(workspace_id, None, None).await?;

        let active_agents = self.list_agents_impl(workspace_id).await?;

        let recent_sessions = self.fetch_recent_sessions(workspace_id, 10).await?;

        Ok(WorkspaceOverview {
            workspace,
            active_threads,
            recent_memories,
            recent_decisions,
            active_agents,
            recent_sessions,
        })
    }

    async fn fetch_thread_sessions(
        &self,
        workspace_id: Uuid,
        thread_id: Uuid,
    ) -> Result<Vec<AgentSession>> {
        let rows = sqlx::query(
            "SELECT id, agent_id, workspace_id, thread_id, started_at, ended_at, summary, findings, files_touched, next_steps, created_at
             FROM agent_sessions WHERE workspace_id = $1 AND thread_id = $2
             ORDER BY started_at DESC LIMIT 10",
        )
        .bind(workspace_id.to_string())
        .bind(thread_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(super::agent::row_to_agent_session)
            .collect()
    }

    async fn fetch_thread_activity(
        &self,
        workspace_id: Uuid,
        thread_id: Uuid,
    ) -> Result<Vec<ActivityEntry>> {
        let rows = sqlx::query(
            "SELECT id, workspace_id, actor, action, entity_type, entity_id, summary, detail, created_at
             FROM activity_log
             WHERE workspace_id = $1 AND entity_id = $2
             ORDER BY created_at DESC LIMIT 20",
        )
        .bind(workspace_id.to_string())
        .bind(thread_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(super::activity::row_to_activity_entry)
            .collect()
    }

    async fn fetch_recent_sessions(
        &self,
        workspace_id: Uuid,
        limit: i64,
    ) -> Result<Vec<AgentSession>> {
        let rows = sqlx::query(
            "SELECT id, agent_id, workspace_id, thread_id, started_at, ended_at, summary, findings, files_touched, next_steps, created_at
             FROM agent_sessions WHERE workspace_id = $1 AND ended_at IS NOT NULL
             ORDER BY ended_at DESC LIMIT $2",
        )
        .bind(workspace_id.to_string())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(super::agent::row_to_agent_session)
            .collect()
    }
}
