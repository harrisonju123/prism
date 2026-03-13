use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn cargo_bin(name: &str) -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".cargo/bin").join(name);
        if p.exists() {
            return p;
        }
    }
    name.into()
}

pub fn prism_binary() -> PathBuf {
    cargo_bin("prism")
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceContext {
    pub workspace: WorkspaceInfo,
    pub active_tasks: Vec<TaskSummary>,
    pub tasks_by_status: Vec<StatusCount>,
    #[serde(default)]
    pub active_agents: Vec<AgentStatus>,
    #[serde(default)]
    pub stale_tasks: Vec<TaskSummary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentStatus {
    pub name: String,
    pub session_open: bool,
    #[serde(default)]
    pub current_task_name: Option<String>,
    #[serde(default)]
    pub current_task_id: Option<String>,
    #[serde(default)]
    pub last_checkin: Option<String>,
}

impl From<prism_context::model::AgentStatus> for AgentStatus {
    fn from(a: prism_context::model::AgentStatus) -> Self {
        Self {
            name: a.name,
            session_open: a.session_open,
            current_task_name: a.current_thread,
            current_task_id: None,
            last_checkin: a.last_checkin.map(|t| t.to_rfc3339()),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionEntry {
    pub id: String,
    pub agent_name: String,
    pub date: String,
    #[serde(default)]
    pub task_name: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    pub action: String,
    pub summary: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceInfo {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub name: String,
    pub status: String,
    pub priority: String,
    #[serde(default)]
    pub epic_name: Option<String>,
    #[serde(default)]
    pub initiative_name: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusCount {
    pub status: String,
    pub count: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskContext {
    pub task: TaskDetail,
    #[serde(default)]
    pub initiative: Option<InitiativeSummary>,
    #[serde(default)]
    pub epic: Option<EpicSummary>,
    #[serde(default)]
    pub blocks: Vec<DependencyInfo>,
    #[serde(default)]
    pub blocked_by: Vec<DependencyInfo>,
    #[serde(default)]
    pub notes: Vec<TaskNote>,
    #[serde(default)]
    pub handoffs: Vec<TaskHandoff>,
    #[serde(default)]
    pub recent_activity: Vec<ActivityEntry>,
    #[serde(default)]
    pub sibling_tasks: Vec<TaskSummary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskDetail {
    pub id: String,
    pub name: String,
    pub status: String,
    pub priority: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub epic_name: Option<String>,
    #[serde(default)]
    pub initiative_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DependencyInfo {
    pub task_id: String,
    pub task_name: String,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InitiativeSummary {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EpicSummary {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskNote {
    pub title: String,
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskHandoff {
    pub agent_name: String,
    pub summary: String,
    #[serde(default)]
    pub next_steps: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActivityEntry {
    pub actor: String,
    pub action: String,
    pub entity_type: String,
    #[serde(default)]
    pub entity_name: Option<String>,
    pub created_at: String,
}
