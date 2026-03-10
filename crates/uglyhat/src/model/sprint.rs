use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sprint {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub goal: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_date: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SprintVelocity {
    pub sprint_id: Uuid,
    pub sprint_name: String,
    pub total_tasks: i64,
    pub done_tasks: i64,
    pub completion_pct: f64,
}
