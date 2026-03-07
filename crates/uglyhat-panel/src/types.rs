use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceContext {
    pub workspace: WorkspaceInfo,
    pub active_tasks: Vec<TaskSummary>,
    pub tasks_by_status: Vec<StatusCount>,
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
