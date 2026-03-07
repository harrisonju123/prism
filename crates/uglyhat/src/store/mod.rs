pub mod sqlite;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::Result;
use crate::model::*;

pub struct BootstrapResult {
    pub workspace: Workspace,
    pub initiative_id: Uuid,
    pub epic_id: Uuid,
    pub api_key: APIKey,
}

#[derive(Debug, Default)]
pub struct TaskFilters {
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub domain: Option<String>,
    pub assignee: Option<String>,
    pub unassigned: Option<bool>,
}

#[derive(Debug, Default)]
pub struct ActivityFilters {
    pub since: Option<DateTime<Utc>>,
    pub actor: Option<String>,
    pub entity_type: Option<String>,
    pub limit: i64,
}

#[derive(Debug, Default)]
pub struct HandoffFilters {
    pub since: Option<DateTime<Utc>>,
    pub agent: Option<String>,
}

#[async_trait]
pub trait Store: Send + Sync {
    // --- Workspace ---
    async fn bootstrap_workspace(
        &self,
        name: &str,
        description: &str,
        key_hash: &str,
        key_prefix: &str,
    ) -> Result<BootstrapResult>;
    async fn get_system_epic_id(&self, workspace_id: Uuid) -> Result<Uuid>;
    async fn create_workspace(
        &self,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Workspace>;
    async fn get_workspace(&self, id: Uuid) -> Result<Workspace>;
    async fn list_workspaces(&self) -> Result<Vec<Workspace>>;
    async fn update_workspace(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Workspace>;
    async fn delete_workspace(&self, id: Uuid) -> Result<()>;

    // --- Initiative ---
    async fn create_initiative(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Initiative>;
    async fn get_initiative(&self, id: Uuid) -> Result<Initiative>;
    async fn list_initiatives_by_workspace(&self, workspace_id: Uuid) -> Result<Vec<Initiative>>;
    async fn update_initiative(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Initiative>;
    async fn delete_initiative(&self, id: Uuid) -> Result<()>;

    // --- Epic ---
    async fn create_epic(
        &self,
        initiative_id: Uuid,
        name: &str,
        description: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Epic>;
    async fn get_epic(&self, id: Uuid) -> Result<Epic>;
    async fn list_epics_by_initiative(&self, initiative_id: Uuid) -> Result<Vec<Epic>>;
    async fn update_epic(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Epic>;
    async fn delete_epic(&self, id: Uuid) -> Result<()>;

    // --- Task ---
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
    ) -> Result<Task>;
    async fn get_task(&self, id: Uuid) -> Result<Task>;
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
    ) -> Result<Task>;
    async fn delete_task(&self, id: Uuid) -> Result<()>;
    async fn list_tasks_by_epic(&self, epic_id: Uuid) -> Result<Vec<Task>>;
    async fn list_tasks_by_workspace(
        &self,
        workspace_id: Uuid,
        filters: TaskFilters,
    ) -> Result<Vec<Task>>;

    // --- Task Context ---
    async fn get_task_context(&self, task_id: Uuid) -> Result<TaskContext>;

    // --- Dependencies ---
    async fn add_dependency(
        &self,
        blocking_task_id: Uuid,
        blocked_task_id: Uuid,
    ) -> Result<TaskDependency>;
    async fn remove_dependency(&self, dep_id: Uuid) -> Result<()>;
    async fn get_dependencies(
        &self,
        task_id: Uuid,
    ) -> Result<(Vec<DependencyInfo>, Vec<DependencyInfo>)>;

    // --- Activity ---
    async fn log_activity(
        &self,
        workspace_id: Uuid,
        actor: &str,
        action: &str,
        entity_type: &str,
        entity_id: Uuid,
        summary: &str,
        detail: Option<serde_json::Value>,
    ) -> Result<()>;
    async fn create_activity(
        &self,
        workspace_id: Uuid,
        actor: &str,
        action: &str,
        entity_type: &str,
        entity_id: Uuid,
        summary: &str,
        detail: Option<serde_json::Value>,
    ) -> Result<ActivityEntry>;
    async fn list_activity(
        &self,
        workspace_id: Uuid,
        filters: ActivityFilters,
    ) -> Result<Vec<ActivityEntry>>;
    async fn list_activity_since(
        &self,
        workspace_id: Uuid,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<ActivityEntry>>;

    // --- Decision ---
    async fn create_decision(
        &self,
        workspace_id: Option<Uuid>,
        initiative_id: Option<Uuid>,
        epic_id: Option<Uuid>,
        title: &str,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Decision>;
    async fn get_decision(&self, id: Uuid) -> Result<Decision>;
    async fn list_decisions_by_workspace(&self, workspace_id: Uuid) -> Result<Vec<Decision>>;
    async fn update_decision(
        &self,
        id: Uuid,
        title: &str,
        content: &str,
        status: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Decision>;
    async fn delete_decision(&self, id: Uuid) -> Result<()>;

    // --- Note ---
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
    ) -> Result<Note>;
    async fn get_note(&self, id: Uuid) -> Result<Note>;
    async fn list_notes_by_parent(&self, parent_type: &str, parent_id: Uuid) -> Result<Vec<Note>>;
    async fn update_note(
        &self,
        id: Uuid,
        title: &str,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Note>;
    async fn delete_note(&self, id: Uuid) -> Result<()>;

    // --- Agent ---
    async fn checkin_agent(
        &self,
        workspace_id: Uuid,
        name: &str,
        capabilities: Vec<String>,
    ) -> Result<CheckinResponse>;
    async fn checkout_agent(
        &self,
        workspace_id: Uuid,
        agent_name: &str,
        summary: &str,
    ) -> Result<AgentSession>;
    async fn list_agents(&self, workspace_id: Uuid) -> Result<Vec<Agent>>;

    // --- Handoff ---
    async fn create_handoff(
        &self,
        task_id: Uuid,
        agent_name: &str,
        summary: &str,
        findings: Vec<String>,
        blockers: Vec<String>,
        next_steps: Vec<String>,
        artifacts: Option<serde_json::Value>,
    ) -> Result<Handoff>;
    async fn get_handoffs_by_task(&self, task_id: Uuid) -> Result<Vec<Handoff>>;
    async fn list_handoffs_by_workspace(
        &self,
        workspace_id: Uuid,
        filters: HandoffFilters,
    ) -> Result<Vec<Handoff>>;
    async fn list_handoffs_by_epic(&self, epic_id: Uuid) -> Result<Vec<Handoff>>;

    // --- API Key ---
    async fn create_api_key(
        &self,
        workspace_id: Uuid,
        name: &str,
        key_hash: &str,
        key_prefix: &str,
    ) -> Result<APIKey>;
    async fn get_api_key_by_hash(&self, key_hash: &str) -> Result<APIKey>;
    async fn list_api_keys_by_workspace(&self, workspace_id: Uuid) -> Result<Vec<APIKey>>;
    async fn delete_api_key(&self, id: Uuid) -> Result<()>;

    // --- Context ---
    async fn get_workspace_context(&self, workspace_id: Uuid) -> Result<WorkspaceContext>;
    async fn get_next_tasks(&self, workspace_id: Uuid, limit: i64) -> Result<Vec<TaskSummary>>;
}
