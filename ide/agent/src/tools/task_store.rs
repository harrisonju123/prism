use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is waiting to be started.
    Pending,
    /// Task is currently being worked on.
    InProgress,
    /// Task has been completed successfully.
    Completed,
    /// Task failed and will not be retried.
    Failed,
    /// Task is blocked by another task.
    Blocked,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Failed => write!(f, "failed"),
            TaskStatus::Blocked => write!(f, "blocked"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEntry {
    pub id: String,
    pub subject: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    /// IDs of tasks that this task blocks (this must complete before those can proceed).
    pub blocks: Vec<String>,
    /// IDs of tasks that this task is blocked by (those must complete before this can proceed).
    pub blocked_by: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl TaskEntry {
    pub fn format(&self) -> String {
        let mut s = format!(
            "[{}] {} (status: {})",
            &self.id[..8],
            self.subject,
            self.status
        );
        if let Some(desc) = &self.description {
            s.push_str(&format!("\n  Description: {}", desc));
        }
        if !self.blocks.is_empty() {
            let ids: Vec<_> = self.blocks.iter().map(|id| &id[..8]).collect();
            s.push_str(&format!("\n  Blocks: {}", ids.join(", ")));
        }
        if !self.blocked_by.is_empty() {
            let ids: Vec<_> = self.blocked_by.iter().map(|id| &id[..8]).collect();
            s.push_str(&format!("\n  Blocked by: {}", ids.join(", ")));
        }
        s
    }
}

#[derive(Debug, Default)]
pub struct TaskStore {
    tasks: Vec<TaskEntry>,
}

impl TaskStore {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::default()))
    }

    pub fn create(&mut self, subject: String, description: Option<String>) -> TaskEntry {
        let entry = TaskEntry {
            id: Uuid::new_v4().to_string(),
            subject,
            description,
            status: TaskStatus::Pending,
            blocks: vec![],
            blocked_by: vec![],
            created_at: Utc::now(),
        };
        self.tasks.push(entry.clone());
        entry
    }

    pub fn get(&self, id: &str) -> Option<&TaskEntry> {
        self.tasks
            .iter()
            .find(|t| t.id == id || t.id.starts_with(id))
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut TaskEntry> {
        self.tasks
            .iter_mut()
            .find(|t| t.id == id || t.id.starts_with(id))
    }

    pub fn tasks(&self) -> &[TaskEntry] {
        &self.tasks
    }
}
