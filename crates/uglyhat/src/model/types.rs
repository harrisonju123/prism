use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::enums::{TaskPriority, TaskStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Initiative {
    pub id: Uuid,
    pub workspace_id: Uuid,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub workspace_name: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Epic {
    pub id: Uuid,
    pub initiative_id: Uuid,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub initiative_name: String,
    pub workspace_id: Uuid,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub workspace_name: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub epic_id: Uuid,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub epic_name: String,
    pub initiative_id: Uuid,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub initiative_name: String,
    pub workspace_id: Uuid,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub workspace_name: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub assignee: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub domain_tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub blocks: Vec<DependencyInfo>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub blocked_by: Vec<DependencyInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<Uuid>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub workspace_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initiative_id: Option<Uuid>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub initiative_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epic_id: Option<Uuid>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub epic_name: String,
    pub title: String,
    pub content: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initiative_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epic_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<Uuid>,
    pub title: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct APIKey {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    #[serde(skip)]
    pub key_hash: String,
    pub key_prefix: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct APIKeyWithRaw {
    #[serde(flatten)]
    pub api_key: APIKey,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapResponse {
    pub workspace: Workspace,
    pub system_initiative_id: Uuid,
    pub system_epic_id: Uuid,
    pub api_key: APIKeyWithRaw,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitiativeWithCounts {
    pub id: Uuid,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    pub status: String,
    pub epic_count: i64,
    pub task_count: i64,
    pub done_count: i64,
    pub progress_pct: f64,
    pub blocked_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: Uuid,
    pub name: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub assignee: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub epic_name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub initiative_name: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub domain_tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusCount {
    pub status: TaskStatus,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorityCount {
    pub priority: TaskPriority,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceContext {
    pub workspace: Workspace,
    pub initiatives: Vec<InitiativeWithCounts>,
    pub active_tasks: Vec<TaskSummary>,
    pub recent_tasks: Vec<TaskSummary>,
    pub decisions: Vec<Decision>,
    pub tasks_by_status: Vec<StatusCount>,
    pub tasks_by_priority: Vec<PriorityCount>,
    pub blocked_tasks_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub id: Uuid,
    pub workspace_id: Uuid,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub actor: String,
    pub action: String,
    pub entity_type: String,
    pub entity_id: Uuid,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDependency {
    pub id: Uuid,
    pub blocking_task_id: Uuid,
    pub blocked_task_id: Uuid,
    pub workspace_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyInfo {
    pub task_id: Uuid,
    pub task_name: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checkin: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub summary: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckinResponse {
    pub agent: Agent,
    pub session: AgentSession,
    pub assigned_tasks: Vec<TaskSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub recent_activity: Vec<ActivityEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    pub task: Task,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initiative: Option<Initiative>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epic: Option<Epic>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub sibling_tasks: Vec<TaskSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub blocks: Vec<DependencyInfo>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub blocked_by: Vec<DependencyInfo>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub notes: Vec<Note>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub epic_decisions: Vec<Decision>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub initiative_decisions: Vec<Decision>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub recent_activity: Vec<ActivityEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub handoffs: Vec<Handoff>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handoff {
    pub id: Uuid,
    pub task_id: Uuid,
    pub workspace_id: Uuid,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub agent_name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub findings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub blockers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub next_steps: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}
