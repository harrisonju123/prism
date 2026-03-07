mod activity;
mod agent;
mod apikey;
mod context;
mod decision;
mod dependency;
mod epic;
mod handoff;
mod initiative;
mod note;
mod task;
mod task_context;
pub mod types;
mod workspace;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

use crate::error::Result;
use crate::model::*;
use crate::store::{ActivityFilters, BootstrapResult, HandoffFilters, Store, TaskFilters};

pub struct SqliteStore {
    pub pool: SqlitePool,
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

        let schema = include_str!("schema.sql");
        sqlx::raw_sql(schema)
            .execute(&pool)
            .await
            .map_err(|e| crate::error::Error::Internal(format!("apply schema: {e}")))?;

        Ok(Self { pool })
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn bootstrap_workspace(
        &self,
        name: &str,
        description: &str,
        key_hash: &str,
        key_prefix: &str,
    ) -> Result<BootstrapResult> {
        self.bootstrap_workspace_impl(name, description, key_hash, key_prefix)
            .await
    }

    async fn get_system_epic_id(&self, workspace_id: Uuid) -> Result<Uuid> {
        self.get_system_epic_id_impl(workspace_id).await
    }

    async fn create_workspace(
        &self,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Workspace> {
        self.create_workspace_impl(name, description, metadata)
            .await
    }

    async fn get_workspace(&self, id: Uuid) -> Result<Workspace> {
        self.get_workspace_impl(id).await
    }

    async fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        self.list_workspaces_impl().await
    }

    async fn update_workspace(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Workspace> {
        self.update_workspace_impl(id, name, description, metadata)
            .await
    }

    async fn delete_workspace(&self, id: Uuid) -> Result<()> {
        self.delete_workspace_impl(id).await
    }

    async fn create_initiative(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Initiative> {
        self.create_initiative_impl(workspace_id, name, description, metadata)
            .await
    }

    async fn get_initiative(&self, id: Uuid) -> Result<Initiative> {
        self.get_initiative_impl(id).await
    }

    async fn list_initiatives_by_workspace(&self, workspace_id: Uuid) -> Result<Vec<Initiative>> {
        self.list_initiatives_by_workspace_impl(workspace_id).await
    }

    async fn update_initiative(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Initiative> {
        self.update_initiative_impl(id, name, description, status, metadata)
            .await
    }

    async fn delete_initiative(&self, id: Uuid) -> Result<()> {
        self.delete_initiative_impl(id).await
    }

    async fn create_epic(
        &self,
        initiative_id: Uuid,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Epic> {
        self.create_epic_impl(initiative_id, name, description, metadata)
            .await
    }

    async fn get_epic(&self, id: Uuid) -> Result<Epic> {
        self.get_epic_impl(id).await
    }

    async fn list_epics_by_initiative(&self, initiative_id: Uuid) -> Result<Vec<Epic>> {
        self.list_epics_by_initiative_impl(initiative_id).await
    }

    async fn update_epic(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Epic> {
        self.update_epic_impl(id, name, description, status, metadata)
            .await
    }

    async fn delete_epic(&self, id: Uuid) -> Result<()> {
        self.delete_epic_impl(id).await
    }

    async fn create_task(
        &self,
        epic_id: Uuid,
        name: &str,
        description: &str,
        status: TaskStatus,
        priority: TaskPriority,
        assignee: &str,
        domain_tags: Vec<String>,
        metadata: Option<serde_json::Value>,
    ) -> Result<Task> {
        self.create_task_impl(
            epic_id,
            name,
            description,
            status,
            priority,
            assignee,
            domain_tags,
            metadata,
        )
        .await
    }

    async fn get_task(&self, id: Uuid) -> Result<Task> {
        self.get_task_impl(id).await
    }

    async fn update_task(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        status: TaskStatus,
        priority: TaskPriority,
        assignee: &str,
        domain_tags: Vec<String>,
        metadata: Option<serde_json::Value>,
    ) -> Result<Task> {
        self.update_task_impl(
            id,
            name,
            description,
            status,
            priority,
            assignee,
            domain_tags,
            metadata,
        )
        .await
    }

    async fn delete_task(&self, id: Uuid) -> Result<()> {
        self.delete_task_impl(id).await
    }

    async fn list_tasks_by_epic(&self, epic_id: Uuid) -> Result<Vec<Task>> {
        self.list_tasks_by_epic_impl(epic_id).await
    }

    async fn list_tasks_by_workspace(
        &self,
        workspace_id: Uuid,
        filters: TaskFilters,
    ) -> Result<Vec<Task>> {
        self.list_tasks_by_workspace_impl(workspace_id, filters)
            .await
    }

    async fn get_task_context(&self, task_id: Uuid) -> Result<TaskContext> {
        self.get_task_context_impl(task_id).await
    }

    async fn add_dependency(
        &self,
        blocking_task_id: Uuid,
        blocked_task_id: Uuid,
    ) -> Result<TaskDependency> {
        self.add_dependency_impl(blocking_task_id, blocked_task_id)
            .await
    }

    async fn remove_dependency(&self, dep_id: Uuid) -> Result<()> {
        self.remove_dependency_impl(dep_id).await
    }

    async fn get_dependencies(
        &self,
        task_id: Uuid,
    ) -> Result<(Vec<DependencyInfo>, Vec<DependencyInfo>)> {
        self.get_dependencies_impl(task_id).await
    }

    async fn log_activity(
        &self,
        workspace_id: Uuid,
        actor: &str,
        action: &str,
        entity_type: &str,
        entity_id: Uuid,
        summary: &str,
        detail: Option<serde_json::Value>,
    ) -> Result<()> {
        self.log_activity_impl(
            workspace_id,
            actor,
            action,
            entity_type,
            entity_id,
            summary,
            detail,
        )
        .await
    }

    async fn create_activity(
        &self,
        workspace_id: Uuid,
        actor: &str,
        action: &str,
        entity_type: &str,
        entity_id: Uuid,
        summary: &str,
        detail: Option<serde_json::Value>,
    ) -> Result<ActivityEntry> {
        self.create_activity_impl(
            workspace_id,
            actor,
            action,
            entity_type,
            entity_id,
            summary,
            detail,
        )
        .await
    }

    async fn list_activity(
        &self,
        workspace_id: Uuid,
        filters: ActivityFilters,
    ) -> Result<Vec<ActivityEntry>> {
        self.list_activity_impl(workspace_id, filters).await
    }

    async fn list_activity_since(
        &self,
        workspace_id: Uuid,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ActivityEntry>> {
        self.list_activity_since_impl(workspace_id, since, limit)
            .await
    }

    async fn create_decision(
        &self,
        workspace_id: Option<Uuid>,
        initiative_id: Option<Uuid>,
        epic_id: Option<Uuid>,
        title: &str,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Decision> {
        self.create_decision_impl(
            workspace_id,
            initiative_id,
            epic_id,
            title,
            content,
            metadata,
        )
        .await
    }

    async fn get_decision(&self, id: Uuid) -> Result<Decision> {
        self.get_decision_impl(id).await
    }

    async fn list_decisions_by_workspace(&self, workspace_id: Uuid) -> Result<Vec<Decision>> {
        self.list_decisions_by_workspace_impl(workspace_id).await
    }

    async fn update_decision(
        &self,
        id: Uuid,
        title: &str,
        content: &str,
        status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Decision> {
        self.update_decision_impl(id, title, content, status, metadata)
            .await
    }

    async fn delete_decision(&self, id: Uuid) -> Result<()> {
        self.delete_decision_impl(id).await
    }

    async fn create_note(
        &self,
        workspace_id: Option<Uuid>,
        initiative_id: Option<Uuid>,
        epic_id: Option<Uuid>,
        task_id: Option<Uuid>,
        decision_id: Option<Uuid>,
        title: &str,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Note> {
        self.create_note_impl(
            workspace_id,
            initiative_id,
            epic_id,
            task_id,
            decision_id,
            title,
            content,
            metadata,
        )
        .await
    }

    async fn get_note(&self, id: Uuid) -> Result<Note> {
        self.get_note_impl(id).await
    }

    async fn list_notes_by_parent(&self, parent_type: &str, parent_id: Uuid) -> Result<Vec<Note>> {
        self.list_notes_by_parent_impl(parent_type, parent_id).await
    }

    async fn update_note(
        &self,
        id: Uuid,
        title: &str,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Note> {
        self.update_note_impl(id, title, content, metadata).await
    }

    async fn delete_note(&self, id: Uuid) -> Result<()> {
        self.delete_note_impl(id).await
    }

    async fn checkin_agent(
        &self,
        workspace_id: Uuid,
        name: &str,
        capabilities: Vec<String>,
    ) -> Result<CheckinResponse> {
        self.checkin_agent_impl(workspace_id, name, capabilities)
            .await
    }

    async fn checkout_agent(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        summary: &str,
    ) -> Result<AgentSession> {
        self.checkout_agent_impl(workspace_id, agent_name, summary)
            .await
    }

    async fn list_agents(&self, workspace_id: Uuid) -> Result<Vec<Agent>> {
        self.list_agents_impl(workspace_id).await
    }

    async fn create_handoff(
        &self,
        task_id: Uuid,
        agent_name: &str,
        summary: &str,
        findings: Vec<String>,
        blockers: Vec<String>,
        next_steps: Vec<String>,
        artifacts: Option<serde_json::Value>,
    ) -> Result<Handoff> {
        self.create_handoff_impl(
            task_id, agent_name, summary, findings, blockers, next_steps, artifacts,
        )
        .await
    }

    async fn get_handoffs_by_task(&self, task_id: Uuid) -> Result<Vec<Handoff>> {
        self.get_handoffs_by_task_impl(task_id).await
    }

    async fn list_handoffs_by_workspace(
        &self,
        workspace_id: Uuid,
        filters: HandoffFilters,
    ) -> Result<Vec<Handoff>> {
        self.list_handoffs_by_workspace_impl(workspace_id, filters)
            .await
    }

    async fn list_handoffs_by_epic(&self, epic_id: Uuid) -> Result<Vec<Handoff>> {
        self.list_handoffs_by_epic_impl(epic_id).await
    }

    async fn create_api_key(
        &self,
        workspace_id: Uuid,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
    ) -> Result<APIKey> {
        self.create_api_key_impl(workspace_id, name, key_hash, key_prefix)
            .await
    }

    async fn get_api_key_by_hash(&self, key_hash: &str) -> Result<APIKey> {
        self.get_api_key_by_hash_impl(key_hash).await
    }

    async fn list_api_keys_by_workspace(&self, workspace_id: Uuid) -> Result<Vec<APIKey>> {
        self.list_api_keys_by_workspace_impl(workspace_id).await
    }

    async fn delete_api_key(&self, id: Uuid) -> Result<()> {
        self.delete_api_key_impl(id).await
    }

    async fn get_workspace_context(&self, workspace_id: Uuid) -> Result<WorkspaceContext> {
        self.get_workspace_context_impl(workspace_id).await
    }

    async fn get_next_tasks(&self, workspace_id: Uuid, limit: i64) -> Result<Vec<TaskSummary>> {
        self.get_next_tasks_impl(workspace_id, limit).await
    }
}
