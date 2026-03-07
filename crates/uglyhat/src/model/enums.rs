use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum TaskStatus {
    #[serde(rename = "backlog")]
    Backlog,
    #[serde(rename = "todo")]
    Todo,
    #[serde(rename = "in_progress")]
    InProgress,
    #[serde(rename = "in_review")]
    InReview,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "cancelled")]
    Cancelled,
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskStatus::Backlog => write!(f, "backlog"),
            TaskStatus::Todo => write!(f, "todo"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::InReview => write!(f, "in_review"),
            TaskStatus::Done => write!(f, "done"),
            TaskStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl TaskStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, TaskStatus::Done | TaskStatus::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum TaskPriority {
    #[serde(rename = "critical")]
    Critical,
    #[serde(rename = "high")]
    High,
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "low")]
    Low,
}

impl fmt::Display for TaskPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskPriority::Critical => write!(f, "critical"),
            TaskPriority::High => write!(f, "high"),
            TaskPriority::Medium => write!(f, "medium"),
            TaskPriority::Low => write!(f, "low"),
        }
    }
}
